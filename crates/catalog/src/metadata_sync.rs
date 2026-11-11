//! The matcher + apply step (MD2).
//!
//! Rules, in order:
//! 1. A provider id written into a folder/file name by an *arr
//!    tool (`{tmdb-603}`, `[tvdbid=78804]`) wins outright — fetch
//!    that record, confidence 1.0.
//! 2. Otherwise search by normalised title + year and score every
//!    candidate. Auto-apply only at >= 0.85 confidence with a
//!    clear margin over the runner-up.
//! 3. A plausible-but-uncertain best (>= 0.45) parks the item in
//!    `needs_review` with its candidates stored for the fix-match
//!    API. Below that, the item stays `unmatched`.
//!
//! The matcher never silently guesses: a wrong match is worse
//! than no match, because it poisons artwork, ratings and watch
//! state downstream.

use serde::Serialize;
use uuid::Uuid;

use parlour_metadata::{
    extract_provider_ids, score_candidate, Details, MediaKind, MetadataProvider, ProviderError,
    AUTO_MATCH_MARGIN, AUTO_MATCH_THRESHOLD, REVIEW_THRESHOLD,
};

use crate::parse::sort_title;

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("no such item")]
    NotFound,
    #[error("item kind {0} does not take metadata directly")]
    Skipped(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchOutcome {
    pub status: String,
    pub provider: Option<String>,
    pub provider_id: Option<i64>,
    pub confidence: Option<f64>,
}

struct ItemRow {
    kind: String,
    title: String,
    year: Option<i32>,
}

async fn load_item(pool: &parlour_db::PgPool, item_id: Uuid) -> Result<ItemRow, MatchError> {
    let row: Option<(String, String, Option<i32>)> =
        sqlx::query_as("SELECT kind, title, year FROM media_items WHERE id = $1")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(kind, title, year)| ItemRow { kind, title, year })
        .ok_or(MatchError::NotFound)
}

fn media_kind(kind: &str) -> Result<MediaKind, MatchError> {
    match kind {
        "movie" => Ok(MediaKind::Movie),
        "series" => Ok(MediaKind::Show),
        other => Err(MatchError::Skipped(other.to_string())),
    }
}

async fn set_status(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    status: &str,
    provider: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE media_items SET metadata_status = $2, metadata_provider = $3,
                last_metadata_at = now() WHERE id = $1",
    )
    .bind(item_id)
    .bind(status)
    .bind(provider)
    .execute(pool)
    .await?;
    Ok(())
}

/// Apply fetched details to an item (and, for a series, episode
/// data to its episode children). Also records provider artwork
/// as `images` rows for the artwork job to cache (MD3).
pub async fn apply_details<P: MetadataProvider>(
    pool: &parlour_db::PgPool,
    provider: &P,
    item_id: Uuid,
    details: &Details,
) -> Result<(), MatchError> {
    let item = load_item(pool, item_id).await?;
    sqlx::query(
        "UPDATE media_items SET
             tmdb_id = COALESCE($2, tmdb_id),
             tvdb_id = COALESCE($3, tvdb_id),
             imdb_id = COALESCE($4, imdb_id),
             overview = COALESCE($5, overview),
             content_rating = COALESCE($6, content_rating),
             genres = CASE WHEN $7::text[] = '{}' THEN genres ELSE $7 END,
             vote_average = COALESCE($8, vote_average),
             runtime_minutes = COALESCE($9, runtime_minutes),
             year = COALESCE(year, $10),
             metadata_status = 'matched', metadata_provider = $11,
             last_metadata_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(item_id)
    .bind(details.tmdb_id)
    .bind(details.tvdb_id)
    .bind(&details.imdb_id)
    .bind(&details.overview)
    .bind(&details.content_rating)
    .bind(&details.genres)
    .bind(details.vote_average)
    .bind(details.runtime_minutes)
    .bind(details.year)
    .bind(&details.provider)
    .execute(pool)
    .await?;

    sqlx::query("DELETE FROM metadata_candidates WHERE item_id = $1")
        .bind(item_id)
        .execute(pool)
        .await?;

    // Cast (people + item_people). Replaced wholesale per fetch.
    sqlx::query("DELETE FROM item_people WHERE item_id = $1 AND role = 'actor'")
        .bind(item_id)
        .execute(pool)
        .await?;
    for member in &details.cast {
        let person_id: (Uuid,) = sqlx::query_as(
            "INSERT INTO people (name, tmdb_id) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE
                 SET tmdb_id = COALESCE(people.tmdb_id, EXCLUDED.tmdb_id)
             RETURNING id",
        )
        .bind(&member.name)
        .bind(if details.provider == "tmdb" {
            member.provider_person_id
        } else {
            None
        })
        .fetch_one(pool)
        .await?;
        sqlx::query(
            "INSERT INTO item_people (item_id, person_id, role, character_name, sort_order)
             VALUES ($1, $2, 'actor', $3, $4)
             ON CONFLICT (item_id, person_id, role)
             DO UPDATE SET character_name = EXCLUDED.character_name,
                           sort_order = EXCLUDED.sort_order",
        )
        .bind(item_id)
        .bind(person_id.0)
        .bind(&member.character)
        .bind(member.order)
        .execute(pool)
        .await?;
    }

    // Artwork rows (the download job fills `path`).
    sqlx::query("DELETE FROM images WHERE item_id = $1 AND source_url IS NOT NULL")
        .bind(item_id)
        .execute(pool)
        .await?;
    for (kind, variant, url) in [
        ("poster", "original", &details.poster_url),
        ("poster", "w300", &details.poster_url),
        ("backdrop", "original", &details.backdrop_url),
        ("logo", "original", &details.logo_url),
    ] {
        if let Some(url) = url {
            sqlx::query(
                "INSERT INTO images (item_id, kind, variant, path, source_url)
                 VALUES ($1, $2, $3, 'pending:' || $2 || ':' || $3, $4)
                 ON CONFLICT (item_id, kind, path) DO NOTHING",
            )
            .bind(item_id)
            .bind(kind)
            .bind(variant)
            .bind(url)
            .execute(pool)
            .await?;
        }
    }

    // Series: pull each season's episodes and enrich the episode
    // items the scanner created.
    if item.kind == "series" {
        let seasons: Vec<(Uuid, i32)> = sqlx::query_as(
            "SELECT id, season_number FROM media_items
             WHERE parent_id = $1 AND kind = 'season' AND season_number IS NOT NULL",
        )
        .bind(item_id)
        .fetch_all(pool)
        .await?;
        for (season_id, season_number) in seasons {
            let episodes = provider
                .season_episodes(details.provider_id, season_number)
                .await?;
            for ep in episodes {
                let air_date = ep
                    .air_date
                    .as_deref()
                    .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
                if let Some(title) = &ep.title {
                    sqlx::query(
                        "UPDATE media_items SET
                             title = $3, sort_title = $4,
                             overview = COALESCE($5, overview),
                             air_date = COALESCE($6, air_date),
                             runtime_minutes = COALESCE($7, runtime_minutes),
                             updated_at = now()
                         WHERE parent_id = $1 AND kind = 'episode' AND episode_number = $2",
                    )
                    .bind(season_id)
                    .bind(ep.episode)
                    .bind(title)
                    .bind(sort_title(title))
                    .bind(&ep.overview)
                    .bind(air_date)
                    .bind(ep.runtime_minutes)
                    .execute(pool)
                    .await?;
                } else {
                    sqlx::query(
                        "UPDATE media_items SET
                             overview = COALESCE($3, overview),
                             air_date = COALESCE($4, air_date),
                             runtime_minutes = COALESCE($5, runtime_minutes),
                             updated_at = now()
                         WHERE parent_id = $1 AND kind = 'episode' AND episode_number = $2",
                    )
                    .bind(season_id)
                    .bind(ep.episode)
                    .bind(&ep.overview)
                    .bind(air_date)
                    .bind(ep.runtime_minutes)
                    .execute(pool)
                    .await?;
                }
                if let Some(still) = &ep.still_url {
                    let ep_row: Option<(Uuid,)> = sqlx::query_as(
                        "SELECT id FROM media_items
                         WHERE parent_id = $1 AND kind = 'episode' AND episode_number = $2",
                    )
                    .bind(season_id)
                    .bind(ep.episode)
                    .fetch_optional(pool)
                    .await?;
                    if let Some((ep_id,)) = ep_row {
                        sqlx::query(
                            "INSERT INTO images (item_id, kind, variant, path, source_url)
                             VALUES ($1, 'still', 'original', 'pending:still:original', $2)
                             ON CONFLICT (item_id, kind, path) DO NOTHING",
                        )
                        .bind(ep_id)
                        .bind(still)
                        .execute(pool)
                        .await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Match one top-level item (movie/series) against a provider.
pub async fn match_item<P: MetadataProvider>(
    pool: &parlour_db::PgPool,
    provider: &P,
    item_id: Uuid,
) -> Result<MatchOutcome, MatchError> {
    let item = load_item(pool, item_id).await?;
    let kind = media_kind(&item.kind)?;

    // Rule 1: explicit provider ids in the file/folder names.
    let rel_paths: Vec<(String,)> =
        sqlx::query_as("SELECT relative_path FROM media_files WHERE item_id = $1")
            .bind(item_id)
            .fetch_all(pool)
            .await?;
    let mut tmdb_id = None;
    let mut tvdb_id = None;
    for (rel,) in &rel_paths {
        let ids = extract_provider_ids(rel);
        tmdb_id = tmdb_id.or(ids.tmdb);
        tvdb_id = tvdb_id.or(ids.tvdb);
    }
    let explicit = match provider.name() {
        "tmdb" => tmdb_id,
        "tvdb" => tvdb_id,
        _ => None,
    };
    if let Some(id) = explicit {
        let details = provider.details(kind, id).await?;
        apply_details(pool, provider, item_id, &details).await?;
        return Ok(MatchOutcome {
            status: "matched".into(),
            provider: Some(provider.name().into()),
            provider_id: Some(id),
            confidence: Some(1.0),
        });
    }

    // Rule 2/3: scored search.
    let mut scored: Vec<(f64, parlour_metadata::Candidate)> = provider
        .search(kind, &item.title, item.year)
        .await?
        .into_iter()
        .map(|c| (score_candidate(&item.title, item.year, &c), c))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let Some((best_score, best)) = scored.first().cloned() else {
        set_status(pool, item_id, "unmatched", Some(provider.name())).await?;
        return Ok(MatchOutcome {
            status: "unmatched".into(),
            provider: Some(provider.name().into()),
            provider_id: None,
            confidence: Some(0.0),
        });
    };
    let runner_up = scored.get(1).map(|(s, _)| *s).unwrap_or(0.0);

    if best_score >= AUTO_MATCH_THRESHOLD && best_score - runner_up >= AUTO_MATCH_MARGIN {
        let details = provider.details(kind, best.provider_id).await?;
        apply_details(pool, provider, item_id, &details).await?;
        return Ok(MatchOutcome {
            status: "matched".into(),
            provider: Some(provider.name().into()),
            provider_id: Some(best.provider_id),
            confidence: Some(best_score),
        });
    }

    if best_score >= REVIEW_THRESHOLD {
        sqlx::query("DELETE FROM metadata_candidates WHERE item_id = $1")
            .bind(item_id)
            .execute(pool)
            .await?;
        for (score, cand) in scored.iter().take(5) {
            sqlx::query(
                "INSERT INTO metadata_candidates
                     (item_id, provider, provider_id, title, year, overview, poster_url, confidence)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (item_id, provider, provider_id) DO NOTHING",
            )
            .bind(item_id)
            .bind(&cand.provider)
            .bind(cand.provider_id)
            .bind(&cand.title)
            .bind(cand.year)
            .bind(&cand.overview)
            .bind(&cand.poster_url)
            .bind(score)
            .execute(pool)
            .await?;
        }
        set_status(pool, item_id, "needs_review", Some(provider.name())).await?;
        return Ok(MatchOutcome {
            status: "needs_review".into(),
            provider: Some(provider.name().into()),
            provider_id: Some(best.provider_id),
            confidence: Some(best_score),
        });
    }

    set_status(pool, item_id, "unmatched", Some(provider.name())).await?;
    Ok(MatchOutcome {
        status: "unmatched".into(),
        provider: Some(provider.name().into()),
        provider_id: Some(best.provider_id),
        confidence: Some(best_score),
    })
}

/// Manual fix (MD4): fetch this exact provider record and apply.
pub async fn apply_manual<P: MetadataProvider>(
    pool: &parlour_db::PgPool,
    provider: &P,
    item_id: Uuid,
    provider_id: i64,
) -> Result<(), MatchError> {
    let item = load_item(pool, item_id).await?;
    let kind = media_kind(&item.kind)?;
    let details = provider.details(kind, provider_id).await?;
    apply_details(pool, provider, item_id, &details).await
}

/// Match every top-level item in a library; summary JSON counts
/// by outcome. Per-item provider failures are counted, not fatal.
pub async fn match_library<P: MetadataProvider>(
    pool: &parlour_db::PgPool,
    provider: &P,
    library_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<serde_json::Value, MatchError> {
    let items: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM media_items
         WHERE library_id = $1 AND parent_id IS NULL AND kind IN ('movie', 'series')
         ORDER BY sort_title",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;
    let total = items.len() as i32;
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }
    let mut counts = serde_json::json!({
        "matched": 0, "needs_review": 0, "unmatched": 0, "failed": 0, "total": total
    });
    let mut done = 0i32;
    for (item_id,) in &items {
        done += 1;
        match match_item(pool, provider, *item_id).await {
            Ok(outcome) => {
                if let Some(n) = counts.get_mut(outcome.status.as_str()) {
                    *n = serde_json::json!(n.as_i64().unwrap_or(0) + 1);
                }
            }
            Err(e) => {
                counts["failed"] = serde_json::json!(counts["failed"].as_i64().unwrap_or(0) + 1);
                if let Some(job) = job_id {
                    let _ = parlour_jobs::add_error(pool, job, None, &e.to_string()).await;
                }
            }
        }
        if let Some(job) = job_id {
            let _ = parlour_jobs::set_progress(pool, job, done, total).await;
        }
    }
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_result(pool, job, counts.clone()).await;
    }
    Ok(counts)
}
