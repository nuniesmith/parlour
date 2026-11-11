//! TMDB API v3 client (MD1).
//!
//! Auth is the v3 API key as a query parameter. Search, movie/TV
//! details (with credits, external ids, certifications), artwork
//! URLs, and per-season episode lists. JSON mapping functions are
//! separate from HTTP so they are fixture-tested.

use serde_json::Value;

use crate::{
    Candidate, CastMember, Details, EpisodeInfo, MediaKind, MetadataProvider, ProviderError,
};

pub const DEFAULT_BASE: &str = "https://api.themoviedb.org/3";
pub const DEFAULT_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/";

#[derive(Clone)]
pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
    base: String,
    image_base: String,
}

impl TmdbClient {
    pub fn new(api_key: String) -> Self {
        Self::with_base(api_key, DEFAULT_BASE)
    }

    pub fn with_base(api_key: String, base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        let image_base = if base == DEFAULT_BASE {
            DEFAULT_IMAGE_BASE.to_string()
        } else {
            format!("{base}/images/")
        };
        Self {
            http: reqwest::Client::new(),
            api_key,
            base,
            image_base,
        }
    }

    async fn get_json(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Value, ProviderError> {
        let mut query: Vec<(&str, String)> = vec![("api_key", self.api_key.clone())];
        query.extend_from_slice(params);
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .query(&query)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(ProviderError::Status(res.status().as_u16()));
        }
        res.json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }
}

fn year_from(date: &Value) -> Option<i32> {
    date.as_str()
        .filter(|s| s.len() >= 4)
        .and_then(|s| s[..4].parse().ok())
}

fn image_path(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|p| p.as_str()).map(str::to_string)
}

pub fn parse_search_results(v: &Value, kind: MediaKind, image_base: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return out;
    };
    for r in results {
        let (title_key, date_key) = match kind {
            MediaKind::Movie => ("title", "release_date"),
            MediaKind::Show => ("name", "first_air_date"),
        };
        let Some(id) = r.get("id").and_then(|i| i.as_i64()) else {
            continue;
        };
        let Some(title) = r.get(title_key).and_then(|t| t.as_str()) else {
            continue;
        };
        out.push(Candidate {
            provider: "tmdb".to_string(),
            provider_id: id,
            title: title.to_string(),
            original_title: r
                .get(match kind {
                    MediaKind::Movie => "original_title",
                    MediaKind::Show => "original_name",
                })
                .and_then(|t| t.as_str())
                .map(str::to_string),
            year: year_from(&r.get(date_key).cloned().unwrap_or(Value::Null)),
            overview: r
                .get("overview")
                .and_then(|o| o.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            poster_url: image_path(r, "poster_path").map(|p| format!("{image_base}original{p}")),
            vote_average: r.get("vote_average").and_then(|x| x.as_f64()),
        });
    }
    out
}

fn content_rating(v: &Value, kind: &str) -> Option<String> {
    // Movies: release_dates (append) keyed by country; TV: content_ratings.
    let key = if kind == "movie" {
        "release_dates"
    } else {
        "content_ratings"
    };
    let results = v.get(key)?.get("results")?.as_array()?;
    let pick = results
        .iter()
        .find(|r| r.get("iso_3166_1").and_then(|c| c.as_str()) == Some("US"))
        .or_else(|| results.first())?;
    if kind == "movie" {
        pick.get("release_dates")?
            .as_array()?
            .iter()
            .filter_map(|d| d.get("certification").and_then(|c| c.as_str()))
            .find(|c| !c.is_empty())
            .map(str::to_string)
    } else {
        pick.get("rating")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(str::to_string)
    }
}

pub fn parse_details(
    v: &Value,
    kind: MediaKind,
    image_base: &str,
) -> Result<Details, ProviderError> {
    let id = v
        .get("id")
        .and_then(|i| i.as_i64())
        .ok_or_else(|| ProviderError::Parse("details without id".into()))?;
    let (title_key, date_key) = match kind {
        MediaKind::Movie => ("title", "release_date"),
        MediaKind::Show => ("name", "first_air_date"),
    };
    let title = v
        .get(title_key)
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::Parse("details without title".into()))?
        .to_string();
    let external = v.get("external_ids");
    let cast = v
        .get("credits")
        .and_then(|c| c.get("cast"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .take(30)
                .filter_map(|p| {
                    Some(CastMember {
                        name: p.get("name")?.as_str()?.to_string(),
                        character: p
                            .get("character")
                            .and_then(|c| c.as_str())
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        order: p.get("order").and_then(|o| o.as_i64()).unwrap_or(0) as i32,
                        provider_person_id: p.get("id").and_then(|i| i.as_i64()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let logo = v
        .get("images")
        .and_then(|i| i.get("logos"))
        .and_then(|l| l.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|l| l.get("iso_639_1").and_then(|x| x.as_str()) == Some("en"))
                .or_else(|| arr.first())
        })
        .and_then(|l| l.get("file_path"))
        .and_then(|p| p.as_str())
        .map(str::to_string);
    Ok(Details {
        provider: "tmdb".to_string(),
        provider_id: id,
        tmdb_id: Some(id),
        tvdb_id: external
            .and_then(|e| e.get("tvdb_id"))
            .and_then(|i| i.as_i64()),
        imdb_id: external
            .and_then(|e| e.get("imdb_id"))
            .and_then(|s| s.as_str())
            .map(str::to_string),
        title,
        year: year_from(&v.get(date_key).cloned().unwrap_or(Value::Null)),
        overview: v
            .get("overview")
            .and_then(|o| o.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        content_rating: content_rating(v, kind.as_str()),
        genres: v
            .get("genres")
            .and_then(|g| g.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|g| g.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        runtime_minutes: match kind {
            MediaKind::Movie => v.get("runtime").and_then(|r| r.as_i64()).map(|r| r as i32),
            MediaKind::Show => v
                .get("episode_run_time")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.as_i64())
                .map(|r| r as i32),
        },
        vote_average: v.get("vote_average").and_then(|x| x.as_f64()),
        poster_url: image_path(v, "poster_path").map(|p| format!("{image_base}original{p}")),
        backdrop_url: image_path(v, "backdrop_path").map(|p| format!("{image_base}original{p}")),
        logo_url: logo.map(|p| format!("{image_base}original{p}")),
        cast,
    })
}

pub fn parse_episodes(v: &Value, image_base: &str) -> Vec<EpisodeInfo> {
    let Some(eps) = v.get("episodes").and_then(|e| e.as_array()) else {
        return Vec::new();
    };
    eps.iter()
        .filter_map(|e| {
            Some(EpisodeInfo {
                season: e.get("season_number")?.as_i64()? as i32,
                episode: e.get("episode_number")?.as_i64()? as i32,
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
                    .get("air_date")
                    .and_then(|d| d.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                runtime_minutes: e.get("runtime").and_then(|r| r.as_i64()).map(|r| r as i32),
                still_url: image_path(e, "still_path").map(|p| format!("{image_base}original{p}")),
            })
        })
        .collect()
}

impl MetadataProvider for TmdbClient {
    fn name(&self) -> &'static str {
        "tmdb"
    }

    async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Vec<Candidate>, ProviderError> {
        let (path, year_param) = match kind {
            MediaKind::Movie => ("/search/movie", "year"),
            MediaKind::Show => ("/search/tv", "first_air_date_year"),
        };
        let mut params = vec![("query", title.to_string())];
        if let Some(y) = year {
            params.push((year_param, y.to_string()));
        }
        let v = self.get_json(path, &params).await?;
        Ok(parse_search_results(&v, kind, &self.image_base))
    }

    async fn details(&self, kind: MediaKind, id: i64) -> Result<Details, ProviderError> {
        let path = match kind {
            MediaKind::Movie => format!("/movie/{id}"),
            MediaKind::Show => format!("/tv/{id}"),
        };
        let v = self
            .get_json(
                &path,
                &[(
                    "append_to_response",
                    "credits,external_ids,release_dates,content_ratings,images".to_string(),
                )],
            )
            .await?;
        parse_details(&v, kind, &self.image_base)
    }

    async fn season_episodes(
        &self,
        show_id: i64,
        season: i32,
    ) -> Result<Vec<EpisodeInfo>, ProviderError> {
        let v = self
            .get_json(&format!("/tv/{show_id}/season/{season}"), &[])
            .await?;
        Ok(parse_episodes(&v, &self.image_base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_movie_search() {
        let v = json!({"results": [
            {"id": 603, "title": "The Matrix", "original_title": "The Matrix",
             "release_date": "1999-03-31", "overview": "A hacker learns.",
             "poster_path": "/abc.jpg", "vote_average": 8.2}
        ]});
        let out = parse_search_results(&v, MediaKind::Movie, DEFAULT_IMAGE_BASE);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].provider_id, 603);
        assert_eq!(out[0].year, Some(1999));
        assert_eq!(
            out[0].poster_url.as_deref(),
            Some("https://image.tmdb.org/t/p/original/abc.jpg")
        );
    }

    #[test]
    fn parses_movie_details_with_rating_and_cast() {
        let v = json!({
            "id": 603, "title": "The Matrix", "release_date": "1999-03-31",
            "overview": "A hacker learns.", "runtime": 136, "vote_average": 8.2,
            "poster_path": "/abc.jpg", "backdrop_path": "/def.jpg",
            "genres": [{"name": "Science Fiction"}],
            "external_ids": {"imdb_id": "tt0133093", "tvdb_id": null},
            "release_dates": {"results": [
                {"iso_3166_1": "CA", "release_dates": [{"certification": "14A"}]},
                {"iso_3166_1": "US", "release_dates": [{"certification": "R"}]}
            ]},
            "credits": {"cast": [
                {"name": "Keanu Reeves", "character": "Neo", "order": 0, "id": 6384}
            ]}
        });
        let d = parse_details(&v, MediaKind::Movie, DEFAULT_IMAGE_BASE).unwrap();
        assert_eq!(d.content_rating.as_deref(), Some("R"));
        assert_eq!(d.genres, vec!["Science Fiction"]);
        assert_eq!(d.imdb_id.as_deref(), Some("tt0133093"));
        assert_eq!(d.cast.len(), 1);
        assert_eq!(d.cast[0].character.as_deref(), Some("Neo"));
        assert_eq!(d.runtime_minutes, Some(136));
    }

    #[test]
    fn parses_episodes() {
        let v = json!({"episodes": [
            {"season_number": 1, "episode_number": 2, "name": "Second",
             "overview": "It continues.", "air_date": "2020-01-08", "runtime": 42,
             "still_path": "/s.jpg"}
        ]});
        let eps = parse_episodes(&v, DEFAULT_IMAGE_BASE);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].episode, 2);
        assert_eq!(eps[0].title.as_deref(), Some("Second"));
    }
}
