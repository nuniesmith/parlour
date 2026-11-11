//! ffprobe ingest (C4).
//!
//! ffprobe runs as a subprocess (the runtime image ships ffmpeg) —
//! no libav bindings, no transcoding engine here. Only the JSON
//! parse is interesting, and it is a pure function
//! (`parse_probe_json`) so the HDR/codec/stream logic is
//! fixture-tested without real media files.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct StreamProbe {
    pub index: i32,
    pub kind: String, // "video" | "audio"
    pub codec: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub hdr_format: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: Option<i32>,
    pub bit_rate: Option<i64>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleProbe {
    pub index: i32,
    pub format: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_forced: bool,
    pub is_default: bool,
    pub is_sdh: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProbeResult {
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    pub streams: Vec<StreamProbe>,
    pub subtitles: Vec<SubtitleProbe>,
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Option<Vec<FfStream>>,
    format: Option<FfFormat>,
}

#[derive(Deserialize)]
struct FfFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct FfStream {
    index: i32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    channels: Option<i32>,
    bit_rate: Option<String>,
    color_transfer: Option<String>,
    side_data_list: Option<Vec<FfSideData>>,
    tags: Option<FfTags>,
    disposition: Option<FfDisposition>,
}

#[derive(Deserialize)]
struct FfSideData {
    side_data_type: Option<String>,
}

#[derive(Deserialize)]
struct FfTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct FfDisposition {
    default: Option<i32>,
    forced: Option<i32>,
}

fn tag(stream: &FfStream, pick: impl Fn(&FfTags) -> Option<String>) -> Option<String> {
    stream.tags.as_ref().and_then(pick)
}

/// HDR flavour of a video stream, when any: Dolby Vision wins over
/// the transfer function (a DV stream also carries an HDR10 base),
/// then HDR10 (PQ), then HLG.
fn hdr_format(stream: &FfStream) -> Option<String> {
    let has_dovi = stream
        .side_data_list
        .as_ref()
        .map(|list| {
            list.iter().any(|s| {
                s.side_data_type
                    .as_deref()
                    .map(|t| t.to_lowercase().contains("dovi"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if has_dovi {
        return Some("Dolby Vision".to_string());
    }
    match stream.color_transfer.as_deref() {
        Some("smpte2084") => Some("HDR10".to_string()),
        Some("arib-std-b67") => Some("HLG".to_string()),
        _ => None,
    }
}

/// Parse `ffprobe -print_format json -show_format -show_streams`
/// output. Pure: unit-tested with fixture JSON.
pub fn parse_probe_json(json: &str) -> Result<ProbeResult, String> {
    let out: FfprobeOutput =
        serde_json::from_str(json).map_err(|e| format!("ffprobe output did not parse: {e}"))?;
    let mut result = ProbeResult::default();
    if let Some(fmt) = &out.format {
        result.container = fmt
            .format_name
            .as_ref()
            .map(|n| n.split(',').next().unwrap_or(n).to_string());
        result.duration_ms = fmt
            .duration
            .as_ref()
            .and_then(|d| d.parse::<f64>().ok())
            .map(|secs| (secs * 1000.0) as i64);
    }
    for stream in out.streams.unwrap_or_default() {
        let kind = stream.codec_type.as_deref().unwrap_or("");
        let is_default = stream
            .disposition
            .as_ref()
            .and_then(|d| d.default)
            .unwrap_or(0)
            == 1;
        match kind {
            "video" | "audio" => result.streams.push(StreamProbe {
                index: stream.index,
                kind: kind.to_string(),
                codec: stream.codec_name.clone(),
                width: stream.width,
                height: stream.height,
                hdr_format: if kind == "video" {
                    hdr_format(&stream)
                } else {
                    None
                },
                language: tag(&stream, |t| t.language.clone()),
                title: tag(&stream, |t| t.title.clone()),
                channels: stream.channels,
                bit_rate: stream.bit_rate.as_ref().and_then(|b| b.parse().ok()),
                is_default,
            }),
            "subtitle" => {
                let title = tag(&stream, |t| t.title.clone());
                let is_sdh = title
                    .as_deref()
                    .map(|t| {
                        let lower = t.to_lowercase();
                        lower.contains("sdh") || lower.contains("hearing impaired")
                    })
                    .unwrap_or(false);
                result.subtitles.push(SubtitleProbe {
                    index: stream.index,
                    format: stream.codec_name.clone(),
                    language: tag(&stream, |t| t.language.clone()),
                    title,
                    is_forced: stream
                        .disposition
                        .as_ref()
                        .and_then(|d| d.forced)
                        .unwrap_or(0)
                        == 1,
                    is_default,
                    is_sdh,
                });
            }
            _ => {} // data/attachment streams are not catalogued
        }
    }
    Ok(result)
}

/// Run ffprobe on one file (`PARLOUR_FFPROBE` overrides the binary
/// name/path; the Docker runtime image installs ffmpeg on PATH).
pub fn probe_file(path: &Path) -> Result<ProbeResult, String> {
    let bin = std::env::var("PARLOUR_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string());
    let output = Command::new(&bin)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("could not run ffprobe ({bin}): {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe failed: {}", stderr.trim()));
    }
    parse_probe_json(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "streams": [
        {"index": 0, "codec_name": "hevc", "codec_type": "video", "width": 3840,
         "height": 2160, "color_transfer": "smpte2084", "bit_rate": "20000000",
         "tags": {"language": "eng"}, "disposition": {"default": 1, "forced": 0}},
        {"index": 1, "codec_name": "truehd", "codec_type": "audio", "channels": 8,
         "tags": {"language": "eng", "title": "English TrueHD"},
         "disposition": {"default": 1, "forced": 0}},
        {"index": 2, "codec_name": "subrip", "codec_type": "subtitle",
         "tags": {"language": "eng", "title": "English"},
         "disposition": {"default": 0, "forced": 0}},
        {"index": 3, "codec_name": "hdmv_pgs_subtitle", "codec_type": "subtitle",
         "tags": {"language": "eng", "title": "English SDH"},
         "disposition": {"default": 0, "forced": 1}}
      ],
      "format": {"format_name": "matroska,webm", "duration": "7320.500000"}
    }"#;

    #[test]
    fn parses_streams_hdr_and_subtitles() {
        let r = parse_probe_json(FIXTURE).unwrap();
        assert_eq!(r.container.as_deref(), Some("matroska"));
        assert_eq!(r.duration_ms, Some(7_320_500));
        assert_eq!(r.streams.len(), 2);
        assert_eq!(r.streams[0].codec.as_deref(), Some("hevc"));
        assert_eq!(r.streams[0].hdr_format.as_deref(), Some("HDR10"));
        assert_eq!(r.streams[1].channels, Some(8));
        assert_eq!(r.subtitles.len(), 2);
        assert_eq!(r.subtitles[0].format.as_deref(), Some("subrip"));
        assert!(r.subtitles[1].is_forced);
        assert!(r.subtitles[1].is_sdh);
    }

    #[test]
    fn dolby_vision_wins_over_pq() {
        let json = r#"{"streams": [{"index": 0, "codec_name": "hevc",
          "codec_type": "video", "color_transfer": "smpte2084",
          "side_data_list": [{"side_data_type": "DOVI configuration record"}]}],
          "format": {}}"#;
        let r = parse_probe_json(json).unwrap();
        assert_eq!(r.streams[0].hdr_format.as_deref(), Some("Dolby Vision"));
    }

    #[test]
    fn bad_json_is_an_error_not_a_panic() {
        assert!(parse_probe_json("not json").is_err());
    }
}
