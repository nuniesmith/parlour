//! Emby cutover (Milestone 1.7): a read-only Emby API client
//! (E1) and the watch-state importer (E2/E3).
//!
//! Matching rule (from the TODO's baked-in research): match by
//! **ProviderIds (TMDB/TVDB/IMDb), never by title**. Emby keys
//! watch state by provider id, so duplicates of one film share
//! state — the importer applies state to every Parlour item with
//! the matching id ("import by title, not by file").
//! Episodes match through their series' provider id plus
//! season/episode numbers.
//!
//! The importer is idempotent and always reports: matched,
//! unmatched (listed by name), ambiguous, and skipped (no state
//! worth carrying). A dry run computes the same report without
//! writing anything.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------
// Client (E1)

#[derive(Debug, Clone, Default)]
pub struct EmbySettings {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

impl EmbySettings {
    pub fn configured(&self) -> bool {
        self.base_url.is_some() && self.api_key.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbyError {
    #[error("Emby is not configured (set PARLOUR_EMBY_URL and PARLOUR_EMBY_API_KEY)")]
    NotConfigured,
    #[error("Emby request failed: {0}")]
    Http(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbyUser {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbyUserData {
    #[serde(rename = "Played", default)]
    pub played: bool,
    #[serde(rename = "PlaybackPositionTicks", default)]
    pub position_ticks: i64,
    #[serde(rename = "PlayCount", default)]
    pub play_count: i32,
    #[serde(rename = "LastPlayedDate")]
    pub last_played: Option<DateTime<Utc>>,
    #[serde(rename = "IsFavorite", default)]
    pub is_favorite: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbyItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "ProductionYear")]
    pub year: Option<i32>,
    #[serde(rename = "ProviderIds", default)]
    pub provider_ids: HashMap<String, String>,
    #[serde(rename = "UserData", default)]
    pub user_data: EmbyUserData,
    #[serde(rename = "ParentIndexNumber")]
    pub season_number: Option<i32>,
    #[serde(rename = "IndexNumber")]
    pub episode_number: Option<i32>,
    #[serde(rename = "SeriesId")]
    pub series_id: Option<String>,
}

#[derive(Deserialize)]
struct ItemsPage {
    #[serde(rename = "Items", default)]
    items: Vec<EmbyItem>,
    #[serde(rename = "TotalRecordCount", default)]
    total: usize,
}

pub struct EmbyClient {
    base: String,
    api_key: String,
    http: reqwest::Client,
}

impl EmbyClient {
    pub fn new(settings: &EmbySettings) -> Result<Self, EmbyError> {
        let (Some(base), Some(key)) = (&settings.base_url, &settings.api_key) else {
            return Err(EmbyError::NotConfigured);
        };
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            api_key: key.clone(),
            http: reqwest::Client::new(),
        })
    }

    /// Test hook: point the client at a stub server.
    pub fn with_base(base: &str, api_key: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, EmbyError> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await
            .map_err(|e| EmbyError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(EmbyError::Http(format!("GET {path} -> {}", resp.status())));
        }
        resp.json::<T>()
            .await
            .map_err(|e| EmbyError::Http(e.to_string()))
    }

    pub async fn list_users(&self) -> Result<Vec<EmbyUser>, EmbyError> {
        self.get_json("/emby/Users").await
    }

    /// Fetch one item by id (used to resolve an episode's series
    /// provider ids).
    pub async fn get_item(&self, user_id: &str, item_id: &str) -> Result<EmbyItem, EmbyError> {
        self.get_json(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields=ProviderIds"
        ))
        .await
    }

    /// Page through every movie/episode/series of one user, with
    /// ProviderIds and UserData attached.
    pub async fn list_items(&self, user_id: &str) -> Result<Vec<EmbyItem>, EmbyError> {
        let page_size = 200;
        let mut start = 0usize;
        let mut all = Vec::new();
        loop {
            let path = format!(
                "/emby/Users/{user_id}/Items?Recursive=true&IncludeItemTypes=Movie,Episode,Series\
                 &Fields=ProviderIds,UserData&StartIndex={start}&Limit={page_size}"
            );
            let page: ItemsPage = self.get_json(&path).await?;
            let got = page.items.len();
            all.extend(page.items);
            start += got;
            if got == 0 || start >= page.total {
                break;
            }
        }
        Ok(all)
    }
}

// ---------------------------------------------------------------------
// Importer (E2 + E3)

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportReport {
    pub dry_run: bool,
    pub emby_items_with_state: usize,
    pub matched: usize,
    pub applied: usize,
    pub unmatched: usize,
    pub ambiguous: usize,
    pub skipped_no_state: usize,
    pub unmatched_items: Vec<String>,
    pub ambiguous_items: Vec<String>,
}

fn provider_i64(ids: &HashMap<String, String>, key: &str) -> Option<i64> {
    ids.get(key).and_then(|v| v.parse::<i64>().ok())
}

fn has_state(data: &EmbyUserData) -> bool {
    data.played || data.position_ticks > 0 || data.is_favorite
}

/// Convert Emby ticks (100 ns) to milliseconds.
pub fn ticks_to_ms(ticks: i64) -> i64 {
    ticks / 10_000
}

struct Candidate {
    item_id: Uuid,
}

/// The match for one Emby item. `ambiguous` means the provider
/// id resolved to items that are not the same title (or not the
/// same series, for episodes) — state is NOT applied then,
/// because guessing would corrupt someone's watch history.
struct MatchOutcome {
    candidates: Vec<Candidate>,
    ambiguous: bool,
}

/// Run one user's import into one Parlour profile.
pub async fn run_import(
    pool: &parlour_db::PgPool,
    client: &EmbyClient,
    emby_user_id: &str,
    profile_id: Uuid,
    dry_run: bool,
) -> Result<ImportReport, EmbyError> {
    let items = client.list_items(emby_user_id).await?;
    let mut report = ImportReport {
        dry_run,
        ..Default::default()
    };

    // Series provider ids, for episode matching.
    let mut series_ids: HashMap<String, HashMap<String, String>> = HashMap::new();
    for item in &items {
        if item.kind == "Series" {
            series_ids.insert(item.id.clone(), item.provider_ids.clone());
        }
    }

    for item in &items {
        if item.kind == "Series" {
            continue; // state lives on movies/episodes
        }
        if !has_state(&item.user_data) {
            report.skipped_no_state += 1;
            continue;
        }
        report.emby_items_with_state += 1;

        let outcome = find_candidates(pool, item, &series_ids).await?;
        if outcome.ambiguous {
            report.ambiguous += 1;
            report.ambiguous_items.push(describe(item));
        } else if outcome.candidates.is_empty() {
            report.unmatched += 1;
            report.unmatched_items.push(describe(item));
        } else {
            // Emby shares state across duplicate copies of one
            // title; apply to all of them.
            report.matched += 1;
            if !dry_run {
                for c in &outcome.candidates {
                    apply_state(pool, profile_id, c.item_id, &item.user_data).await?;
                    report.applied += 1;
                }
            }
        }
    }
    Ok(report)
}

fn describe(item: &EmbyItem) -> String {
    match item.year {
        Some(y) => format!("{} ({})", item.name, y),
        None => item.name.clone(),
    }
}

async fn find_candidates(
    pool: &parlour_db::PgPool,
    item: &EmbyItem,
    series_ids: &HashMap<String, HashMap<String, String>>,
) -> Result<MatchOutcome, EmbyError> {
    let tmdb = provider_i64(&item.provider_ids, "Tmdb");
    let tvdb = provider_i64(&item.provider_ids, "Tvdb");
    let imdb = item.provider_ids.get("Imdb").cloned();
    let none = MatchOutcome {
        candidates: Vec::new(),
        ambiguous: false,
    };

    if item.kind == "Episode" {
        // Match the series by its provider ids, then the episode
        // by season/episode number within it.
        let Some(series_key) = &item.series_id else {
            return Ok(none);
        };
        let empty = HashMap::new();
        let series_provider = series_ids.get(series_key).unwrap_or(&empty);
        let s_tmdb = provider_i64(series_provider, "Tmdb");
        let s_tvdb = provider_i64(series_provider, "Tvdb");
        let (Some(season), Some(episode)) = (item.season_number, item.episode_number) else {
            return Ok(none);
        };
        if s_tmdb.is_none() && s_tvdb.is_none() {
            return Ok(none);
        }
        let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT ep.id, sr.id FROM media_items ep
             JOIN media_items sn ON sn.id = ep.parent_id
             JOIN media_items sr ON sr.id = sn.parent_id
             WHERE ep.kind = 'episode' AND ep.season_number = $1 AND ep.episode_number = $2
               AND (($3::bigint IS NOT NULL AND sr.tmdb_id = $3)
                 OR ($4::bigint IS NOT NULL AND sr.tvdb_id = $4))",
        )
        .bind(season)
        .bind(episode)
        .bind(s_tmdb)
        .bind(s_tvdb)
        .fetch_all(pool)
        .await?;
        let series: std::collections::HashSet<Uuid> = rows.iter().map(|(_, sr)| *sr).collect();
        return Ok(MatchOutcome {
            ambiguous: series.len() > 1,
            candidates: rows
                .into_iter()
                .map(|(item_id, _)| Candidate { item_id })
                .collect(),
        });
    }

    // Movies (and anything else): direct provider-id match.
    if tmdb.is_none() && tvdb.is_none() && imdb.is_none() {
        return Ok(none);
    }
    let rows: Vec<(Uuid, String, Option<i32>)> = sqlx::query_as(
        "SELECT id, title, year FROM media_items
         WHERE parent_id IS NULL
           AND (($1::bigint IS NOT NULL AND tmdb_id = $1)
             OR ($2::bigint IS NOT NULL AND tvdb_id = $2)
             OR ($3::text IS NOT NULL AND imdb_id = $3))",
    )
    .bind(tmdb)
    .bind(tvdb)
    .bind(imdb)
    .fetch_all(pool)
    .await?;
    let titles: std::collections::HashSet<(String, Option<i32>)> = rows
        .iter()
        .map(|(_, title, year)| (title.to_lowercase(), *year))
        .collect();
    Ok(MatchOutcome {
        ambiguous: titles.len() > 1,
        candidates: rows
            .into_iter()
            .map(|(item_id, _, _)| Candidate { item_id })
            .collect(),
    })
}

async fn apply_state(
    pool: &parlour_db::PgPool,
    profile_id: Uuid,
    item_id: Uuid,
    data: &EmbyUserData,
) -> Result<(), EmbyError> {
    sqlx::query(
        "INSERT INTO user_data (profile_id, item_id, position_ms, played, is_favorite, last_played_at, play_count, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         ON CONFLICT (profile_id, item_id) DO UPDATE SET
            position_ms = EXCLUDED.position_ms,
            played = EXCLUDED.played,
            is_favorite = EXCLUDED.is_favorite,
            last_played_at = COALESCE(EXCLUDED.last_played_at, user_data.last_played_at),
            play_count = GREATEST(user_data.play_count, EXCLUDED.play_count),
            updated_at = now()",
    )
    .bind(profile_id)
    .bind(item_id)
    .bind(ticks_to_ms(data.position_ticks))
    .bind(data.played)
    .bind(data.is_favorite)
    .bind(data.last_played)
    .bind(data.play_count)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_convert() {
        assert_eq!(ticks_to_ms(36_000_000_000), 3_600_000); // 1 h
        assert_eq!(ticks_to_ms(0), 0);
    }

    #[test]
    fn user_data_parses_with_defaults() {
        let v: EmbyUserData = serde_json::from_value(serde_json::json!({
            "Played": true,
            "PlaybackPositionTicks": 123450000_i64
        }))
        .unwrap();
        assert!(v.played);
        assert_eq!(v.play_count, 0);
        assert!(!v.is_favorite);
    }
}
