//! An episode's chapters, for the player (POD6; POD5 reading 12).
//!
//! Two sources, both read by the server, never by a page:
//!
//! - **The download's own chapters** (ID3 `CHAP`, MP4 chapters), read
//!   with ffprobe (`audiobooks::probe_audio`). No publisher is asked.
//! - **The feed's Podcasting 2.0 chapter list** (`podcast:chapters`, a
//!   JSON file at `podcast_episodes.chapters_url`), fetched when the
//!   player asks. The address is a publisher's and may hold a paid
//!   feed's token, so it is never in an answer, an error or the log.
//!
//! Only start times and titles are kept. A chapter's image and link
//! (publisher addresses) are dropped, and so is a chapter marked
//! `"toc": false` (the JSON format's "not for the table of contents").

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The largest chapter file read.
pub const MAX_CHAPTERS_BYTES: usize = 1024 * 1024;
/// How long one chapter fetch may take.
pub const CHAPTERS_TIMEOUT: Duration = Duration::from_secs(15);
/// The most chapters kept.
pub const MAX_CHAPTERS: usize = 1000;

/// One chapter, on the episode's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpisodeChapter {
    pub start_ms: i64,
    pub title: String,
}

#[derive(Deserialize)]
struct JsonChapters {
    #[serde(default)]
    chapters: Vec<JsonChapter>,
}

#[derive(Deserialize)]
struct JsonChapter {
    #[serde(rename = "startTime")]
    start_time: Option<serde_json::Value>,
    title: Option<serde_json::Value>,
    toc: Option<bool>,
}

fn seconds(v: &serde_json::Value) -> Option<f64> {
    let s = match v {
        serde_json::Value::Number(n) => n.as_f64()?,
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (s.is_finite() && s >= 0.0).then_some(s)
}

/// Read a Podcasting 2.0 JSON chapter list: chapters with a start time
/// and a title, `toc: false` ones left out, in start order, the first of
/// any two at the same time kept.
pub fn parse_json_chapters(bytes: &[u8]) -> Result<Vec<EpisodeChapter>, String> {
    let doc: JsonChapters = serde_json::from_slice(bytes)
        .map_err(|_| "the chapter list isn't Podcasting 2.0 JSON".to_string())?;
    let mut out: Vec<EpisodeChapter> = doc
        .chapters
        .into_iter()
        .filter(|c| c.toc != Some(false))
        .filter_map(|c| {
            let start = seconds(c.start_time.as_ref()?)?;
            let title = match c.title? {
                serde_json::Value::String(s) => s.trim().to_string(),
                _ => return None,
            };
            (!title.is_empty()).then(|| EpisodeChapter {
                start_ms: (start * 1000.0).round() as i64,
                title,
            })
        })
        .collect();
    out.sort_by_key(|c| c.start_ms);
    out.dedup_by_key(|c| c.start_ms);
    out.truncate(MAX_CHAPTERS);
    Ok(out)
}

/// Chapters from a probed file's own chapter marks (titled ones only).
pub fn from_file(chapters: &[crate::audiobooks::FileChapter]) -> Vec<EpisodeChapter> {
    let mut out: Vec<EpisodeChapter> = chapters
        .iter()
        .filter_map(|c| {
            let title = c.title.as_deref()?.trim();
            (!title.is_empty()).then(|| EpisodeChapter {
                start_ms: c.start_ms.max(0),
                title: title.to_string(),
            })
        })
        .collect();
    out.sort_by_key(|c| c.start_ms);
    out.dedup_by_key(|c| c.start_ms);
    out.truncate(MAX_CHAPTERS);
    out
}

fn request_error(e: reqwest::Error) -> String {
    let e = e.without_url();
    if e.is_timeout() {
        "it took too long".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        e.to_string()
    }
}

/// Fetch and read the feed's chapter list at `url`. The error never
/// holds the address.
pub async fn fetch_json_chapters(
    http: &reqwest::Client,
    url: &str,
) -> Result<Vec<EpisodeChapter>, String> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| "the feed's chapter link isn't a web address".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("the feed's chapter link isn't a web address".to_string());
    }
    let mut res = http
        .get(parsed)
        .timeout(CHAPTERS_TIMEOUT)
        .header(
            reqwest::header::ACCEPT,
            "application/json+chapters, application/json;q=0.9, */*;q=0.1",
        )
        .send()
        .await
        .map_err(request_error)?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(format!("the chapter list answered HTTP {status}"));
    }
    let too_large = || {
        format!(
            "the chapter list is larger than {} MB",
            MAX_CHAPTERS_BYTES / (1024 * 1024)
        )
    };
    if res
        .content_length()
        .is_some_and(|n| n as usize > MAX_CHAPTERS_BYTES)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(request_error)? {
        if bytes.len() + chunk.len() > MAX_CHAPTERS_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_json_chapters(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiobooks::FileChapter;

    #[test]
    fn json_chapters_keep_start_and_title_only() {
        let json = br#"{
            "version": "1.2.0",
            "chapters": [
                {"startTime": 95.5, "title": "  The middle  ", "img": "https://cdn.example/a.jpg"},
                {"startTime": 0, "title": "Cold open", "url": "https://sponsor.example/?token=fake-feed-token"},
                {"startTime": 30, "title": "Hidden", "toc": false},
                {"startTime": "120", "title": "Text time"},
                {"startTime": 200},
                {"title": "No time"},
                {"startTime": -4, "title": "Negative"},
                {"startTime": 95.5, "title": "Same time, later"},
                {"startTime": 300, "title": ""}
            ]
        }"#;
        let got = parse_json_chapters(json).unwrap();
        assert_eq!(
            got,
            vec![
                EpisodeChapter {
                    start_ms: 0,
                    title: "Cold open".into()
                },
                EpisodeChapter {
                    start_ms: 95_500,
                    title: "The middle".into()
                },
                EpisodeChapter {
                    start_ms: 120_000,
                    title: "Text time".into()
                },
            ]
        );
        let text = serde_json::to_string(&got).unwrap();
        assert!(!text.contains("http"), "{text}");
    }

    #[test]
    fn bad_json_is_an_error_without_detail() {
        let e = parse_json_chapters(b"<html>nope</html>").unwrap_err();
        assert_eq!(e, "the chapter list isn't Podcasting 2.0 JSON");
        assert_eq!(parse_json_chapters(b"{}").unwrap(), vec![]);
    }

    #[test]
    fn file_chapters_need_a_title() {
        let got = from_file(&[
            FileChapter {
                start_ms: 60_000,
                end_ms: 120_000,
                title: Some("Two".into()),
            },
            FileChapter {
                start_ms: 0,
                end_ms: 60_000,
                title: Some(" One ".into()),
            },
            FileChapter {
                start_ms: 120_000,
                end_ms: 130_000,
                title: None,
            },
        ]);
        assert_eq!(
            got,
            vec![
                EpisodeChapter {
                    start_ms: 0,
                    title: "One".into()
                },
                EpisodeChapter {
                    start_ms: 60_000,
                    title: "Two".into()
                },
            ]
        );
    }
}
