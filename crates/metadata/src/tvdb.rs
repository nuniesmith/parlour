//! TheTVDB API v4 client (MD5).
//!
//! The second provider behind the same trait: TMDB stays primary,
//! TVDB is there for episode ordering/metadata where TMDB is thin.
//! Auth is an API key exchanged for a bearer token (`POST /login`),
//! cached and refreshed once on a 401. Without a key the client
//! is simply never constructed — callers treat TVDB as optional.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{Candidate, Details, EpisodeInfo, MediaKind, MetadataProvider, ProviderError};

pub const DEFAULT_BASE: &str = "https://api4.thetvdb.com/v4";

#[derive(Clone)]
pub struct TvdbClient {
    http: reqwest::Client,
    api_key: String,
    base: String,
    token: Arc<Mutex<Option<String>>>,
}

impl TvdbClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base(api_key, DEFAULT_BASE)
    }

    pub fn with_base(api_key: String, base: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            base: base.trim_end_matches('/').to_string(),
            token: Arc::new(Mutex::new(None)),
        }
    }

    async fn login(&self) -> Result<String, ProviderError> {
        let res = self
            .http
            .post(format!("{}/login", self.base))
            .json(&json!({ "apikey": self.api_key }))
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(ProviderError::Status(res.status().as_u16()));
        }
        let v: Value = res
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        let token = v
            .get("data")
            .and_then(|d| d.get("token"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| ProviderError::Parse("login without token".into()))?
            .to_string();
        *self.token.lock().await = Some(token.clone());
        Ok(token)
    }

    async fn get_json(&self, path: &str) -> Result<Value, ProviderError> {
        for attempt in 0..2 {
            let token = match self.token.lock().await.clone() {
                Some(t) => t,
                None => self.login().await?,
            };
            let res = self
                .http
                .get(format!("{}{path}", self.base))
                .bearer_auth(&token)
                .send()
                .await?;
            if res.status().as_u16() == 401 && attempt == 0 {
                *self.token.lock().await = None;
                continue;
            }
            if !res.status().is_success() {
                return Err(ProviderError::Status(res.status().as_u16()));
            }
            return res
                .json()
                .await
                .map_err(|e| ProviderError::Parse(e.to_string()));
        }
        Err(ProviderError::Other("tvdb auth failed".into()))
    }
}

fn parse_id(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    let s = v.as_str()?;
    if let Ok(n) = s.parse() {
        return Some(n);
    }
    // objectID form: "movie-12345"
    s.rsplit('-').next().and_then(|n| n.parse().ok())
}

pub fn parse_search_results(v: &Value) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(results) = v.get("data").and_then(|d| d.as_array()) else {
        return out;
    };
    for r in results {
        let id = r
            .get("tvdb_id")
            .and_then(parse_id)
            .or_else(|| r.get("id").and_then(parse_id))
            .or_else(|| r.get("objectID").and_then(parse_id));
        let Some(id) = id else { continue };
        let Some(name) = r.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        out.push(Candidate {
            provider: "tvdb".to_string(),
            provider_id: id,
            title: name.to_string(),
            original_title: None,
            year: r.get("year").and_then(|y| {
                y.as_i64()
                    .map(|n| n as i32)
                    .or_else(|| y.as_str()?.parse().ok())
            }),
            overview: r
                .get("overview")
                .and_then(|o| o.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            poster_url: r
                .get("image_url")
                .and_then(|u| u.as_str())
                .map(str::to_string),
            vote_average: None,
        });
    }
    out
}

pub fn parse_details(v: &Value, _kind: MediaKind) -> Result<Details, ProviderError> {
    let d = v
        .get("data")
        .ok_or_else(|| ProviderError::Parse("tvdb details without data".into()))?;
    let id = d
        .get("id")
        .and_then(parse_id)
        .ok_or_else(|| ProviderError::Parse("tvdb details without id".into()))?;
    let title = d
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| ProviderError::Parse("tvdb details without name".into()))?
        .to_string();
    Ok(Details {
        provider: "tvdb".to_string(),
        provider_id: id,
        tmdb_id: None,
        tvdb_id: Some(id),
        imdb_id: None,
        title,
        year: d.get("year").and_then(|y| {
            y.as_i64()
                .map(|n| n as i32)
                .or_else(|| y.as_str()?.parse().ok())
        }),
        overview: d
            .get("overview")
            .and_then(|o| o.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        content_rating: None,
        genres: d
            .get("genres")
            .and_then(|g| g.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|g| {
                        g.as_str()
                            .map(str::to_string)
                            .or_else(|| g.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        runtime_minutes: d.get("runtime").and_then(|r| r.as_i64()).map(|r| r as i32),
        vote_average: None,
        poster_url: d.get("image").and_then(|u| u.as_str()).map(str::to_string),
        backdrop_url: None,
        logo_url: None,
        // TVDB character data rides a separate characters endpoint;
        // episode data (the reason TVDB is here) is in parse_episodes.
        cast: Vec::new(),
    })
}

pub fn parse_episodes(v: &Value) -> Vec<EpisodeInfo> {
    let eps = v
        .get("data")
        .and_then(|d| d.get("episodes"))
        .and_then(|e| e.as_array());
    let Some(eps) = eps else { return Vec::new() };
    eps.iter()
        .filter_map(|e| {
            Some(EpisodeInfo {
                season: e.get("seasonNumber")?.as_i64()? as i32,
                episode: e.get("number")?.as_i64()? as i32,
                title: e
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                overview: e
                    .get("overview")
                    .and_then(|o| o.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                air_date: e
                    .get("aired")
                    .and_then(|d| d.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                runtime_minutes: e.get("runtime").and_then(|r| r.as_i64()).map(|r| r as i32),
                still_url: e.get("image").and_then(|u| u.as_str()).map(str::to_string),
            })
        })
        .collect()
}

impl MetadataProvider for TvdbClient {
    fn name(&self) -> &'static str {
        "tvdb"
    }

    async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Vec<Candidate>, ProviderError> {
        let type_ = match kind {
            MediaKind::Movie => "movie",
            MediaKind::Show => "series",
        };
        let q = urlencoding(title);
        let mut path = format!("/search?query={q}&type={type_}");
        if let Some(y) = year {
            path.push_str(&format!("&year={y}"));
        }
        let v = self.get_json(&path).await?;
        Ok(parse_search_results(&v))
    }

    async fn details(&self, kind: MediaKind, id: i64) -> Result<Details, ProviderError> {
        let path = match kind {
            MediaKind::Movie => format!("/movies/{id}"),
            MediaKind::Show => format!("/series/{id}"),
        };
        let v = self.get_json(&path).await?;
        parse_details(&v, kind)
    }

    async fn season_episodes(
        &self,
        show_id: i64,
        season: i32,
    ) -> Result<Vec<EpisodeInfo>, ProviderError> {
        let v = self
            .get_json(&format!(
                "/series/{show_id}/episodes/default?season={season}"
            ))
            .await
            .unwrap_or(Value::Null);
        Ok(parse_episodes(&v)
            .into_iter()
            .filter(|e| e.season == season)
            .collect())
    }
}

/// Minimal percent-encoding for query values (titles are the only
/// free text we send).
fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_search_and_details() {
        let v = json!({"data": [
            {"tvdb_id": 78804, "name": "Some Show", "year": "2020",
             "overview": "A show.", "image_url": "https://img/x.jpg"}
        ]});
        let out = parse_search_results(&v);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].provider_id, 78804);
        assert_eq!(out[0].year, Some(2020));

        let v = json!({"data": {"id": 78804, "name": "Some Show", "year": 2020,
            "overview": "A show.", "image": "https://img/x.jpg", "runtime": 45,
            "genres": [{"name": "Drama"}]}});
        let d = parse_details(&v, MediaKind::Show).unwrap();
        assert_eq!(d.tvdb_id, Some(78804));
        assert_eq!(d.genres, vec!["Drama"]);
    }

    #[test]
    fn parses_episode_list() {
        let v = json!({"data": {"episodes": [
            {"seasonNumber": 1, "number": 3, "name": "Third", "aired": "2020-02-01",
             "runtime": 44, "image": "https://img/e.jpg", "overview": "Ep."}
        ]}});
        let eps = parse_episodes(&v);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].episode, 3);
        assert_eq!(eps[0].air_date.as_deref(), Some("2020-02-01"));
    }
}
