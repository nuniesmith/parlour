//! POD7 (D31): searching Apple's podcast directory — the iTunes Search
//! API, which needs no key — so an admin can add a show from a result
//! instead of typing its feed address. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Directory search (POD7)".
//!
//! Only the server talks to Apple: a page never calls Apple's API, and
//! the admin's query is sent nowhere else. The query is checked first
//! (`check_query`): a feed address typed into the search box is refused
//! before anything is sent, because a paid feed's address carries a
//! private token that must not be handed to a third party. The search
//! has a timeout and a size limit, and its errors drop the request's
//! URL (which holds the query) through `without_url`.
//!
//! The JSON mapping (`parse_results`) is pure and fixture-tested; the
//! client (`search`) only fetches.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// Apple's iTunes Search API. Tests point `PARLOUR_APPLE_SEARCH_BASE`
/// at a stub on the loopback instead.
pub const APPLE_SEARCH_BASE: &str = "https://itunes.apple.com";
/// How many results one search asks for. Apple allows up to 200.
pub const SEARCH_LIMIT: usize = 25;
/// How long one search may take, all told.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
/// An answer larger than this is refused rather than read into memory.
/// Twenty-five results are well under 100 kB.
pub const MAX_ANSWER_BYTES: usize = 2 * 1024 * 1024;
/// The longest query accepted.
pub const MAX_QUERY_CHARS: usize = 200;
/// The store country searched when none is given.
pub const DEFAULT_COUNTRY: &str = "US";
/// Apple's artwork CDN. Result artwork from anywhere else is dropped.
pub const APPLE_ARTWORK_DOMAIN: &str = "mzstatic.com";

/// One show Apple's directory knows, as the search page needs it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirectoryShow {
    /// Apple's id for the show (`collectionId`), kept in
    /// `podcasts.itunes_id` when the show is added from a result.
    pub itunes_id: i64,
    pub title: String,
    pub author: Option<String>,
    /// The feed's address. `None` when Apple lists none, or one that
    /// isn't an `http`/`https` address: such a result can't be added.
    pub feed_url: Option<String>,
    /// Apple's artwork for the show, on Apple's CDN only.
    pub artwork_url: Option<String>,
    /// Apple's genres, without its catch-all "Podcasts".
    pub genres: Vec<String>,
    /// How many episodes Apple counts (`trackCount`).
    pub episode_count: Option<i64>,
    /// Apple marks the show explicit.
    pub explicit: bool,
    /// When Apple last saw a new episode (`releaseDate`).
    pub latest_episode_at: Option<DateTime<Utc>>,
}

/// Why a search failed, in words that never repeat the query or an
/// address.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DirectoryError {
    #[error("Apple's podcast directory couldn't be reached: {0}")]
    Request(String),
    #[error("Apple's podcast directory answered HTTP {0}")]
    Status(u16),
    #[error("Apple's podcast directory sent an answer larger than {} MB", MAX_ANSWER_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("Apple's podcast directory sent an answer Parlour can't read")]
    Unreadable,
}

/// A query as an admin typed it, checked: white space folded, not
/// empty, at most `MAX_QUERY_CHARS`, and not a web address. A refusal
/// never repeats the query.
pub fn check_query(raw: &str) -> Result<String, String> {
    let query = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return Err("Enter a show's name, or its host's, to search for.".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("A search is at most {MAX_QUERY_CHARS} characters."));
    }
    if looks_like_address(&query) {
        return Err(
            "That looks like a feed's address, so it wasn't sent to Apple. \
                    Add it under \u{201c}Add a show by its address\u{201d} instead."
                .into(),
        );
    }
    Ok(query)
}

/// A query that is, or starts like, a web address: it may be a paid
/// feed's address with its token, which must never reach Apple.
fn looks_like_address(query: &str) -> bool {
    let lower = query.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("feed:")
        || lower.starts_with("itpc://")
        || lower.starts_with("pcast://")
        || lower.starts_with("www.")
    {
        return true;
    }
    // A single word with a dot and a slash or a query string, such as
    // `feeds.example.com/show?token=…`.
    !query.contains(' ') && query.contains('.') && (query.contains('/') || query.contains('?'))
}

/// A store country: two letters, upper-cased; blank or absent is the
/// default (`US`).
pub fn check_country(raw: Option<&str>) -> Result<String, String> {
    let c = raw.map(str::trim).unwrap_or("");
    if c.is_empty() {
        return Ok(DEFAULT_COUNTRY.to_string());
    }
    if c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()) {
        Ok(c.to_ascii_uppercase())
    } else {
        Err("A store country is two letters, such as US, CA or GB.".into())
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// An id Apple sends as a number (or, defensively, as digits).
fn positive_id(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| *n > 0)
}

/// `scheme://host:port` of an address, for comparing origins.
fn origin(url: &reqwest::Url) -> Option<(String, String, Option<u16>)> {
    Some((
        url.scheme().to_string(),
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default(),
    ))
}

/// Result artwork is kept only when it is on Apple's CDN over https —
/// or, so a test's stub can stand in for Apple, on the same origin as
/// the search base. Anything else (a publisher's address) is dropped:
/// pages never load a publisher's artwork (POD4 reading 11).
fn allowed_artwork(raw: &str, base: Option<&reqwest::Url>) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    if url.scheme() == "https"
        && (host == APPLE_ARTWORK_DOMAIN || host.ends_with(&format!(".{APPLE_ARTWORK_DOMAIN}")))
    {
        return true;
    }
    base.is_some_and(|b| b.host_str().is_some() && origin(b) == origin(&url))
}

/// Read Apple's answer (`{"resultCount", "results": [...]}`) into shows,
/// in Apple's order. A result without a positive `collectionId` or a
/// title is skipped, and so is an id seen before. `base` is the search
/// base the answer came from (see `allowed_artwork`).
pub fn parse_results(bytes: &[u8], base: &str) -> Result<Vec<DirectoryShow>, DirectoryError> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|_| DirectoryError::Unreadable)?;
    let results = doc
        .get("results")
        .and_then(Value::as_array)
        .ok_or(DirectoryError::Unreadable)?;
    let base = reqwest::Url::parse(base).ok();
    let mut out: Vec<DirectoryShow> = Vec::new();
    for r in results {
        // A podcast search answers with podcasts; anything else Apple
        // mixes in (an episode, an artist) is not a show.
        if let Some(kind) = r.get("kind").and_then(Value::as_str) {
            if kind != "podcast" {
                continue;
            }
        }
        let Some(itunes_id) = positive_id(r, "collectionId").or_else(|| positive_id(r, "trackId"))
        else {
            continue;
        };
        if out.iter().any(|s| s.itunes_id == itunes_id) {
            continue;
        }
        let Some(title) = text(r, "collectionName").or_else(|| text(r, "trackName")) else {
            continue;
        };
        let feed_url = r
            .get("feedUrl")
            .and_then(Value::as_str)
            .and_then(|u| crate::podcasts::check_feed_url(u).ok());
        let artwork_url = ["artworkUrl600", "artworkUrl100"]
            .iter()
            .filter_map(|k| r.get(*k).and_then(Value::as_str).map(str::trim))
            .find(|u| allowed_artwork(u, base.as_ref()))
            .map(str::to_string);
        let mut genres: Vec<String> = Vec::new();
        let listed = r
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string);
        for g in listed.chain(text(r, "primaryGenreName")) {
            let g = g.trim();
            if !g.is_empty()
                && !g.eq_ignore_ascii_case("podcasts")
                && !genres.iter().any(|o| o.eq_ignore_ascii_case(g))
            {
                genres.push(g.to_string());
            }
        }
        let explicit = r
            .get("collectionExplicitness")
            .and_then(Value::as_str)
            .is_some_and(|e| e.eq_ignore_ascii_case("explicit"))
            || r.get("contentAdvisoryRating")
                .and_then(Value::as_str)
                .is_some_and(|e| e.eq_ignore_ascii_case("explicit"));
        let latest_episode_at = r
            .get("releaseDate")
            .and_then(Value::as_str)
            .and_then(|d| DateTime::parse_from_rfc3339(d.trim()).ok())
            .map(|d| d.with_timezone(&Utc));
        out.push(DirectoryShow {
            itunes_id,
            title,
            author: text(r, "artistName"),
            feed_url,
            artwork_url,
            genres,
            episode_count: r
                .get("trackCount")
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0),
            explicit,
            latest_episode_at,
        });
    }
    Ok(out)
}

fn request_error(e: reqwest::Error) -> DirectoryError {
    // The URL holds the query, so it is dropped from the message.
    let e = e.without_url();
    let detail = if e.is_timeout() {
        "it took too long".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        e.to_string()
    };
    DirectoryError::Request(detail)
}

/// Search Apple's directory for podcasts matching `query` (already
/// checked with `check_query`) in the store `country`.
pub async fn search(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    country: &str,
) -> Result<Vec<DirectoryShow>, DirectoryError> {
    use reqwest::header::{ACCEPT, CONTENT_LENGTH};
    let url = format!("{}/search", base.trim_end_matches('/'));
    let limit = SEARCH_LIMIT.to_string();
    let mut res = http
        .get(url)
        .query(&[
            ("term", query),
            ("media", "podcast"),
            ("entity", "podcast"),
            ("country", country),
            ("limit", limit.as_str()),
        ])
        .header(ACCEPT, "application/json, text/javascript;q=0.9, */*;q=0.5")
        .timeout(SEARCH_TIMEOUT)
        .send()
        .await
        .map_err(request_error)?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(DirectoryError::Status(status));
    }
    let declared = res
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_ANSWER_BYTES) {
        return Err(DirectoryError::TooLarge);
    }
    let mut bytes: Vec<u8> = Vec::with_capacity(declared.unwrap_or(0).min(MAX_ANSWER_BYTES));
    while let Some(chunk) = res.chunk().await.map_err(request_error)? {
        if bytes.len() + chunk.len() > MAX_ANSWER_BYTES {
            return Err(DirectoryError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_results(&bytes, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = include_str!("../tests/fixtures/podcasts/directory-search.json");

    #[test]
    fn queries_are_checked_without_being_repeated() {
        assert_eq!(check_query("  night   owls \n").unwrap(), "night owls");
        for bad in ["", "   ", "\n\t"] {
            assert!(check_query(bad).unwrap_err().contains("Enter"));
        }
        let long = "a".repeat(MAX_QUERY_CHARS + 1);
        let err = check_query(&long).unwrap_err();
        assert!(!err.contains(&long));
        assert!(check_query(&"a".repeat(MAX_QUERY_CHARS)).is_ok());
        for address in [
            "https://feeds.example/show.xml?token=fake-feed-token",
            "HTTP://feeds.example/x",
            "feed://feeds.example/x",
            "itpc://feeds.example/x",
            "www.example.com",
            "feeds.example.com/show?token=fake-feed-token",
            "example.org/rss",
        ] {
            let err = check_query(address).unwrap_err();
            assert!(err.contains("feed's address"), "{address}: {err}");
            assert!(!err.contains("fake-feed-token"));
        }
        // Ordinary searches with dots or slashes are not addresses.
        for ok in [
            "99% Invisible",
            "Mr. Robot recap",
            "AC/DC stories",
            "this.week",
        ] {
            assert!(check_query(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn countries_are_two_letters() {
        assert_eq!(check_country(None).unwrap(), "US");
        assert_eq!(check_country(Some("  ")).unwrap(), "US");
        assert_eq!(check_country(Some("ca")).unwrap(), "CA");
        for bad in ["USA", "u", "1A", "é", "U S"] {
            assert!(check_country(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn apple_answer_is_read_in_order() {
        let shows = parse_results(ANSWER.as_bytes(), APPLE_SEARCH_BASE).unwrap();
        let titles: Vec<&str> = shows.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "The Lantern Hour",
                "Tide Tables",
                "No Feed Here",
                "Loud Kitchen",
                "Track Name Only"
            ]
        );
        let lantern = &shows[0];
        assert_eq!(lantern.itunes_id, 1_000_000_001);
        assert_eq!(lantern.author.as_deref(), Some("Ada Example"));
        assert_eq!(
            lantern.feed_url.as_deref(),
            Some("https://feeds.example/lantern.xml")
        );
        assert_eq!(
            lantern.artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/lantern/600x600bb.jpg")
        );
        assert_eq!(lantern.genres, ["History", "Education"]);
        assert_eq!(lantern.episode_count, Some(42));
        assert!(!lantern.explicit);
        assert_eq!(
            lantern.latest_episode_at.unwrap().to_rfc3339(),
            "2026-11-20T07:00:00+00:00"
        );
        // A publisher's artwork address is dropped; the 100 px one on
        // Apple's CDN is used instead.
        assert_eq!(
            shows[1].artwork_url.as_deref(),
            Some("https://is3-ssl.mzstatic.com/image/thumb/tide/100x100bb.jpg")
        );
        // No feed address, or one that isn't http(s): can't be added.
        assert_eq!(shows[2].feed_url, None);
        assert_eq!(shows[2].artwork_url, None, "plain-http CDN is dropped");
        assert!(shows[3].explicit);
        assert_eq!(shows[3].feed_url, None, "ftp feed address");
        assert_eq!(shows[3].genres, ["Comedy"], "primary genre only");
        assert_eq!(shows[4].itunes_id, 1_000_000_009, "trackId stands in");
        assert_eq!(shows[4].episode_count, None);
        assert!(shows[4].genres.is_empty());
    }

    #[test]
    fn a_stub_base_may_serve_its_own_artwork() {
        let body = br#"{"results":[
            {"kind":"podcast","collectionId":7,"collectionName":"Stub",
             "artworkUrl600":"http://127.0.0.1:4100/art/7.png"},
            {"kind":"podcast","collectionId":8,"collectionName":"Elsewhere",
             "artworkUrl600":"http://127.0.0.1:4200/art/8.png"}]}"#;
        let shows = parse_results(body, "http://127.0.0.1:4100").unwrap();
        assert_eq!(
            shows[0].artwork_url.as_deref(),
            Some("http://127.0.0.1:4100/art/7.png")
        );
        assert_eq!(shows[1].artwork_url, None, "another port is another origin");
        // Against Apple's own base, the stub's address is a stranger.
        let shows = parse_results(body, APPLE_SEARCH_BASE).unwrap();
        assert_eq!(shows[0].artwork_url, None);
    }

    #[test]
    fn unreadable_answers_are_refused() {
        for bad in [&b"<html>busy</html>"[..], b"{}", b"{\"results\": 3}", b""] {
            assert_eq!(
                parse_results(bad, APPLE_SEARCH_BASE),
                Err(DirectoryError::Unreadable)
            );
        }
        assert_eq!(
            parse_results(br#"{"resultCount":0,"results":[]}"#, APPLE_SEARCH_BASE).unwrap(),
            vec![]
        );
    }
}
