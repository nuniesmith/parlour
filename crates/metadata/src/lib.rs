//! Parlour metadata providers (Milestone 1.4).
//!
//! Providers sit behind the `MetadataProvider` trait so a new
//! library type means adding a provider, not touching the
//! catalog. TMDB (`tmdb`) is primary; TheTVDB (`tvdb`) is the
//! second provider behind the same trait (MD5), used where TMDB
//! episode data is thin. Later, with their library types:
//! MusicBrainz (music), Audnexus (audiobooks), OpenLibrary
//! (ebooks).
//!
//! This crate is pure I/O against provider APIs plus pure matching
//! math (`score_candidate`, `extract_provider_ids`) — applying
//! results to the database lives in `parlour-catalog`.

pub mod tmdb;
pub mod tvdb;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Movie,
    Show,
}

impl MediaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Movie => "movie",
            MediaKind::Show => "show",
        }
    }
}

/// One search result, normalised across providers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub provider: String,
    pub provider_id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub vote_average: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CastMember {
    pub name: String,
    pub character: Option<String>,
    pub order: i32,
    pub provider_person_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EpisodeInfo {
    pub season: i32,
    pub episode: i32,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub runtime_minutes: Option<i32>,
    pub still_url: Option<String>,
}

/// Full details for one movie/show, normalised across providers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Details {
    pub provider: String,
    pub provider_id: i64,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub content_rating: Option<String>,
    pub genres: Vec<String>,
    pub runtime_minutes: Option<i32>,
    pub vote_average: Option<f64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub logo_url: Option<String>,
    pub cast: Vec<CastMember>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("provider returned HTTP {0}")]
    Status(u16),
    #[error("provider response did not parse: {0}")]
    Parse(String),
    #[error("provider is not configured (no API key)")]
    MissingKey,
    #[error("{0}")]
    Other(String),
}

/// A metadata source the catalog can match against.
pub trait MetadataProvider: Send + Sync {
    /// Stable short name, e.g. "tmdb".
    fn name(&self) -> &'static str;

    fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> impl std::future::Future<Output = Result<Vec<Candidate>, ProviderError>> + Send;

    fn details(
        &self,
        kind: MediaKind,
        id: i64,
    ) -> impl std::future::Future<Output = Result<Details, ProviderError>> + Send;

    /// Episodes of one season of a show (empty for providers/kinds
    /// that do not offer them).
    fn season_episodes(
        &self,
        _show_id: i64,
        _season: i32,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeInfo>, ProviderError>> + Send {
        async { Ok(Vec::new()) }
    }
}

/// The configured providers, TMDB preferred. Implements the same
/// trait by delegation so callers stay provider-agnostic.
#[derive(Clone)]
pub enum AnyProvider {
    Tmdb(tmdb::TmdbClient),
    Tvdb(tvdb::TvdbClient),
}

impl MetadataProvider for AnyProvider {
    fn name(&self) -> &'static str {
        match self {
            AnyProvider::Tmdb(c) => c.name(),
            AnyProvider::Tvdb(c) => c.name(),
        }
    }

    async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Vec<Candidate>, ProviderError> {
        match self {
            AnyProvider::Tmdb(c) => c.search(kind, title, year).await,
            AnyProvider::Tvdb(c) => c.search(kind, title, year).await,
        }
    }

    async fn details(&self, kind: MediaKind, id: i64) -> Result<Details, ProviderError> {
        match self {
            AnyProvider::Tmdb(c) => c.details(kind, id).await,
            AnyProvider::Tvdb(c) => c.details(kind, id).await,
        }
    }

    async fn season_episodes(
        &self,
        show_id: i64,
        season: i32,
    ) -> Result<Vec<EpisodeInfo>, ProviderError> {
        match self {
            AnyProvider::Tmdb(c) => c.season_episodes(show_id, season).await,
            AnyProvider::Tvdb(c) => c.season_episodes(show_id, season).await,
        }
    }
}

// ---------------------------------------------------------------- matching

/// Normalise a title for comparison: lowercase alphanumeric tokens.
pub fn normalize_title(s: &str) -> String {
    let mut out = String::new();
    let mut last_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

fn token_set(s: &str) -> std::collections::HashSet<String> {
    normalize_title(s)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Title similarity in [0, 1]: exact normalised match = 1, one
/// containing the other = 0.9, else token Jaccard.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let (na, nb) = (normalize_title(a), normalize_title(b));
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    if na.contains(&nb) || nb.contains(&na) {
        return 0.9;
    }
    let (sa, sb) = (token_set(a), token_set(b));
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Confidence that `cand` is the item described by a parsed file
/// name. 0.75 title + 0.25 year (unknown year scores half).
pub fn score_candidate(query_title: &str, query_year: Option<i32>, cand: &Candidate) -> f64 {
    let mut title = title_similarity(query_title, &cand.title);
    if let Some(orig) = &cand.original_title {
        title = title.max(title_similarity(query_title, orig));
    }
    let year_score = match (query_year, cand.year) {
        (Some(q), Some(c)) if q == c => 1.0,
        (Some(q), Some(c)) if (q - c).abs() == 1 => 0.4,
        (Some(_), Some(_)) => 0.0,
        _ => 0.5,
    };
    0.75 * title + 0.25 * year_score
}

/// At or above this confidence (with a clear margin) a match is
/// applied automatically.
pub const AUTO_MATCH_THRESHOLD: f64 = 0.85;
/// At or above this (but not auto-matchable) the item goes to the
/// manual review queue with its candidates stored.
pub const REVIEW_THRESHOLD: f64 = 0.45;
/// How far ahead of the runner-up an auto-match must be.
pub const AUTO_MATCH_MARGIN: f64 = 0.05;

// ---------------------------------------------------------------- explicit ids

/// Provider ids written into folder/file names by *arr tools:
/// `{tmdb-12345}`, `[tmdbid=12345]`, `{tvdb-678}`, `tt1234567`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderIds {
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub imdb: Option<String>,
}

fn scan_id_after(text: &str, marker: &str) -> Option<i64> {
    let lower = text.to_lowercase();
    let start = lower.find(marker)? + marker.len();
    let rest = &lower[start..];
    let rest = rest.strip_prefix("id").unwrap_or(rest);
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

pub fn extract_provider_ids(text: &str) -> ProviderIds {
    let mut ids = ProviderIds {
        tmdb: scan_id_after(text, "tmdb"),
        tvdb: scan_id_after(text, "tvdb"),
        imdb: None,
    };
    // IMDb ids are "tt" + 7-9 digits as their own token.
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b't' && bytes[i + 1] == b't' && bytes[i + 2].is_ascii_digit() {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if before_ok {
                let digits: String = lower[i + 2..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if (7..=9).contains(&digits.len()) {
                    ids.imdb = Some(format!("tt{digits}"));
                    break;
                }
            }
        }
        i += 1;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(title: &str, year: Option<i32>) -> Candidate {
        Candidate {
            provider: "tmdb".into(),
            provider_id: 1,
            title: title.into(),
            original_title: None,
            year,
            overview: None,
            poster_url: None,
            vote_average: None,
        }
    }

    #[test]
    fn scoring_rewards_exact_title_and_year() {
        assert_eq!(
            score_candidate("Alien", Some(1979), &cand("Alien", Some(1979))),
            1.0
        );
        let wrong_year = score_candidate("Alien", Some(1979), &cand("Alien", Some(1986)));
        assert!(wrong_year < AUTO_MATCH_THRESHOLD, "{wrong_year}");
        let wrong_title = score_candidate("Alien", Some(1979), &cand("Predator", Some(1987)));
        assert!(wrong_title < REVIEW_THRESHOLD, "{wrong_title}");
        let no_year = score_candidate("Alien", None, &cand("Alien", Some(1979)));
        assert!(no_year >= AUTO_MATCH_THRESHOLD, "{no_year}");
    }

    #[test]
    fn title_similarity_cases() {
        assert_eq!(title_similarity("The Matrix", "the matrix!"), 1.0);
        assert!(title_similarity("Star Wars", "Star Wars Rebels") >= 0.9);
        assert!(title_similarity("Star Wars", "Star Trek") < 0.5);
    }

    #[test]
    fn extracts_ids_from_arr_names() {
        let ids = extract_provider_ids("Some Movie (2019) {tmdb-603}");
        assert_eq!(ids.tmdb, Some(603));
        let ids = extract_provider_ids("Show [tvdbid=78804] tt0944947");
        assert_eq!(ids.tvdb, Some(78804));
        assert_eq!(ids.imdb.as_deref(), Some("tt0944947"));
        let ids = extract_provider_ids("Plain Name (2000)");
        assert_eq!(ids, ProviderIds::default());
    }
}
