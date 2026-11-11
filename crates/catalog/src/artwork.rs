//! Artwork caching (MD3).
//!
//! Provider artwork URLs land on `images` rows at match time;
//! this module downloads them into the local cache at two sizes
//! (the provider's original, plus a w300 variant for grids — TMDB
//! serves sized variants from the same CDN path, so no image
//! processing happens here). Items with no poster at all get a
//! generated SVG tile (title + initial), so the UI never shows a
//! broken image.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Default, Serialize)]
pub struct ArtworkReport {
    pub downloaded: i32,
    pub skipped: i32,
    pub generated: i32,
    pub errors: Vec<String>,
}

fn sized_url(source: &str, variant: &str) -> String {
    if variant == "original" {
        return source.to_string();
    }
    // TMDB CDN: /t/p/original/<file> -> /t/p/<variant>/<file>.
    // Other providers: the same URL at every variant.
    source.replace("/original/", &format!("/{variant}/"))
}

fn extension_of(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url);
    match clean.rsplit('.').next() {
        Some(e) if e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()) => e.to_lowercase(),
        _ => "jpg".to_string(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The generated placeholder tile for an item without a poster.
pub fn generated_tile_svg(title: &str) -> String {
    // A stable pleasant colour from the title hash.
    let digest = sha256_hex(title.as_bytes());
    let hue = u32::from_str_radix(&digest[..4], 16).unwrap_or(210) % 360;
    let initial = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"500\" height=\"750\" viewBox=\"0 0 500 750\">\
         <rect width=\"500\" height=\"750\" fill=\"hsl({hue},35%,32%)\"/>\
         <text x=\"250\" y=\"360\" font-family=\"sans-serif\" font-size=\"220\" fill=\"#ffffff\" \
         text-anchor=\"middle\" dominant-baseline=\"middle\">{}</text>\
         <text x=\"250\" y=\"690\" font-family=\"sans-serif\" font-size=\"30\" fill=\"#dddddd\" \
         text-anchor=\"middle\">{}</text></svg>",
        xml_escape(&initial),
        xml_escape(title)
    )
}

/// Download (or generate) artwork for one item.
pub async fn download_item_artwork(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    cache_dir: &Path,
    item_id: Uuid,
) -> Result<ArtworkReport, sqlx::Error> {
    let mut report = ArtworkReport::default();
    let rows: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
        "SELECT id, kind, variant, source_url, path FROM images
         WHERE item_id = $1 AND source_url IS NOT NULL ORDER BY kind, variant",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;

    for (image_id, kind, variant, source_url, path) in &rows {
        if !path.is_empty() && Path::new(path).exists() {
            report.skipped += 1;
            continue;
        }
        let url = sized_url(source_url, variant);
        let bytes = match http.get(&url).send().await {
            Ok(res) if res.status().is_success() => match res.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    report.errors.push(format!("{url}: {e}"));
                    continue;
                }
            },
            Ok(res) => {
                report
                    .errors
                    .push(format!("{url}: HTTP {}", res.status().as_u16()));
                continue;
            }
            Err(e) => {
                report.errors.push(format!("{url}: {e}"));
                continue;
            }
        };
        let dir: PathBuf = cache_dir.join(item_id.to_string());
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            report.errors.push(format!("cache dir: {e}"));
            continue;
        }
        let dest = dir.join(format!("{kind}-{variant}.{}", extension_of(&url)));
        if let Err(e) = tokio::fs::write(&dest, &bytes).await {
            report.errors.push(format!("{}: {e}", dest.display()));
            continue;
        }
        sqlx::query("UPDATE images SET path = $2, hash = $3 WHERE id = $1")
            .bind(image_id)
            .bind(dest.to_string_lossy().to_string())
            .bind(sha256_hex(&bytes))
            .execute(pool)
            .await?;
        report.downloaded += 1;
    }

    // No poster on disk at all -> generated tile.
    let poster_on_disk: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM images WHERE item_id = $1 AND kind = 'poster' AND path <> '' AND path NOT LIKE 'pending:%')",
    )
    .bind(item_id)
    .fetch_one(pool)
    .await?;
    if !poster_on_disk.0 {
        let item: Option<(String,)> = sqlx::query_as("SELECT title FROM media_items WHERE id = $1")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
        if let Some((title,)) = item {
            let dir = cache_dir.join(item_id.to_string());
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(sqlx::Error::Io)?;
            let dest = dir.join("poster-generated.svg");
            let svg = generated_tile_svg(&title);
            tokio::fs::write(&dest, svg.as_bytes())
                .await
                .map_err(sqlx::Error::Io)?;
            sqlx::query(
                "INSERT INTO images (item_id, kind, variant, path, hash)
                 VALUES ($1, 'poster', 'generated', $2, $3)
                 ON CONFLICT (item_id, kind, path) DO NOTHING",
            )
            .bind(item_id)
            .bind(dest.to_string_lossy().to_string())
            .bind(sha256_hex(svg.as_bytes()))
            .execute(pool)
            .await?;
            report.generated += 1;
        }
    }
    Ok(report)
}

/// Artwork for every item in a library (top-level items carry the
/// posters/backdrops; episode stills follow their episodes).
pub async fn download_library_artwork(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    cache_dir: &Path,
    library_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<serde_json::Value, sqlx::Error> {
    let items: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM media_items WHERE library_id = $1 ORDER BY sort_title")
            .bind(library_id)
            .fetch_all(pool)
            .await?;
    let total = items.len() as i32;
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }
    let (mut downloaded, mut skipped, mut generated, mut failed) = (0, 0, 0, 0);
    let mut done = 0i32;
    for (item_id,) in &items {
        done += 1;
        match download_item_artwork(pool, http, cache_dir, *item_id).await {
            Ok(r) => {
                downloaded += r.downloaded;
                skipped += r.skipped;
                generated += r.generated;
                if !r.errors.is_empty() {
                    failed += 1;
                    if let Some(job) = job_id {
                        for e in &r.errors {
                            let _ = parlour_jobs::add_error(pool, job, None, e).await;
                        }
                    }
                }
            }
            Err(e) => {
                failed += 1;
                if let Some(job) = job_id {
                    let _ = parlour_jobs::add_error(pool, job, None, &e.to_string()).await;
                }
            }
        }
        if let Some(job) = job_id {
            let _ = parlour_jobs::set_progress(pool, job, done, total).await;
        }
    }
    let summary = serde_json::json!({
        "downloaded": downloaded, "skipped": skipped,
        "generated": generated, "failed_items": failed, "total_items": total
    });
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_result(pool, job, summary.clone()).await;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_urls_rewrite_tmdb_variants() {
        let src = "https://image.tmdb.org/t/p/original/abc.jpg";
        assert_eq!(sized_url(src, "original"), src);
        assert_eq!(
            sized_url(src, "w300"),
            "https://image.tmdb.org/t/p/w300/abc.jpg"
        );
        let other = "https://img.example/poster.jpg";
        assert_eq!(sized_url(other, "w300"), other);
    }

    #[test]
    fn generated_tile_contains_title_and_initial() {
        let svg = generated_tile_svg("Test Movie");
        assert!(svg.contains(">T<"), "{svg}");
        assert!(svg.contains("Test Movie"));
        assert!(generated_tile_svg("A & B").contains("A &amp; B"));
    }
}
