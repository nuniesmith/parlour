//! A show's artwork, cached by Parlour (POD6; POD4 reading 11).
//!
//! A feed names its artwork by address (`podcasts.image_url`). Pages
//! never load that address: it would hand every listener's IP to the
//! publisher (and a paid feed's artwork address may carry its token).
//! Instead the server fetches the image once and keeps it in the
//! artwork cache (`PARLOUR_ARTWORK_DIR/podcasts/<show>/`), recorded in
//! `podcasts.image_path`, and the listener API serves it from there
//! (`GET /api/v1/podcasts/{id}/artwork`).
//!
//! - **When.** After every feed check of the show (scheduled or "refresh
//!   now") and right after an admin adds it. A show whose cached copy is
//!   for its current address is left alone, so a check costs nothing
//!   once the image is cached.
//! - **Which file.** The file is named after a hash of the address it
//!   came from (`cover-<16 hex>.<ext>`), so a feed that changes its
//!   artwork address gets the new image at its next check and the old
//!   file is removed. A feed that drops its artwork drops the copy.
//! - **What is kept.** Only a JPEG, PNG, WebP or GIF, recognised by its
//!   first bytes, at most 20 MB. Anything else (an SVG, an HTML error
//!   page) is refused: the cache serves from Parlour's own origin.
//! - **Errors** never repeat the address (reqwest's `without_url`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use uuid::Uuid;

use crate::artwork::sha256_hex;

/// The largest artwork kept.
pub const MAX_ARTWORK_BYTES: usize = 20 * 1024 * 1024;
/// How long one artwork fetch may take.
pub const ARTWORK_TIMEOUT: Duration = Duration::from_secs(60);

/// What one call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkOutcome {
    /// The cached copy is for the feed's current address.
    Unchanged,
    /// A new copy was fetched and recorded.
    Cached,
    /// The feed names no artwork any more; the copy was removed.
    Cleared,
    /// The feed names no artwork, and there was no copy.
    NoArtwork,
    /// The fetch failed (the reason never holds the address); any older
    /// copy is kept.
    Failed(String),
    /// No such show.
    NoShow,
}

/// The image kinds kept, from their first bytes: the extension to store
/// them under. `None` for anything else.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

/// The content type a cached file is served with, from its extension;
/// `None` for anything this module wouldn't have written.
pub fn content_type_of(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()? {
        "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// The key a copy of `image_url` is stored under: the first 16 hex
/// digits of the address's SHA-256 (it says nothing of the address).
pub fn artwork_key(image_url: &str) -> String {
    sha256_hex(image_url.trim().as_bytes())[..16].to_string()
}

/// The key of a stored copy, from its file name (`cover-<key>.<ext>`).
pub fn key_of_path(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    let rest = name.strip_prefix("cover-")?;
    let (key, _ext) = rest.split_once('.')?;
    (key.len() == 16 && key.chars().all(|c| c.is_ascii_hexdigit())).then_some(key)
}

/// Where a listener's page loads a show's artwork from: Parlour's own
/// route, versioned by the copy's key so a browser keeps it until it
/// changes. `None` when nothing is cached.
pub fn artwork_route(podcast_id: Uuid, image_path: Option<&str>) -> Option<String> {
    let key = key_of_path(image_path?)?;
    Some(format!("/api/v1/podcasts/{podcast_id}/artwork?v={key}"))
}

/// The folder a show's artwork is kept in.
pub fn show_dir(artwork_dir: &Path, podcast_id: Uuid) -> PathBuf {
    artwork_dir.join("podcasts").join(podcast_id.to_string())
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

/// Fetch the image at `url`: a 200 whose body is a kept image kind, at
/// most [`MAX_ARTWORK_BYTES`]. The error never holds the address.
pub async fn fetch_artwork(
    http: &reqwest::Client,
    url: &str,
) -> Result<(Vec<u8>, &'static str), String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| "not a web address".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("not a web address".to_string());
    }
    let mut res = http
        .get(parsed)
        .timeout(ARTWORK_TIMEOUT)
        .header(
            reqwest::header::ACCEPT,
            "image/jpeg, image/png, image/webp, image/gif;q=0.9, */*;q=0.1",
        )
        .send()
        .await
        .map_err(request_error)?;
    let status = res.status().as_u16();
    if status != 200 {
        return Err(format!("the artwork answered HTTP {status}"));
    }
    if res
        .content_length()
        .is_some_and(|n| n as usize > MAX_ARTWORK_BYTES)
    {
        return Err(too_large());
    }
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(request_error)? {
        if bytes.len() + chunk.len() > MAX_ARTWORK_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    let ext =
        sniff_image(&bytes).ok_or_else(|| "it isn't a JPEG, PNG, WebP or GIF image".to_string())?;
    Ok((bytes, ext))
}

fn too_large() -> String {
    format!(
        "the artwork is larger than {} MB",
        MAX_ARTWORK_BYTES / (1024 * 1024)
    )
}

/// Bring a show's cached artwork in line with its feed's artwork
/// address (see the module notes). Only a database error is an `Err`.
pub async fn cache_show_artwork(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    podcast_id: Uuid,
) -> Result<ArtworkOutcome, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT image_url, image_path FROM podcasts WHERE id = $1")
            .bind(podcast_id)
            .fetch_optional(pool)
            .await?;
    let Some((image_url, image_path)) = row else {
        return Ok(ArtworkOutcome::NoShow);
    };
    let image_url = image_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    let Some(image_url) = image_url else {
        let Some(old) = image_path else {
            return Ok(ArtworkOutcome::NoArtwork);
        };
        let done = sqlx::query(
            "UPDATE podcasts SET image_path = NULL
             WHERE id = $1 AND image_path = $2 AND image_url IS NULL",
        )
        .bind(podcast_id)
        .bind(&old)
        .execute(pool)
        .await?;
        if done.rows_affected() == 1 {
            let _ = tokio::fs::remove_file(&old).await;
        }
        return Ok(ArtworkOutcome::Cleared);
    };
    let key = artwork_key(&image_url);
    if let Some(path) = image_path.as_deref() {
        if key_of_path(path) == Some(key.as_str()) && Path::new(path).is_file() {
            return Ok(ArtworkOutcome::Unchanged);
        }
    }
    let (bytes, ext) = match fetch_artwork(http, &image_url).await {
        Ok(ok) => ok,
        Err(e) => return Ok(ArtworkOutcome::Failed(e)),
    };
    let dir = show_dir(artwork_dir, podcast_id);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Ok(ArtworkOutcome::Failed(format!(
            "the artwork cache can't be written: {e}"
        )));
    }
    let dest = dir.join(format!("cover-{key}.{ext}"));
    let temp = dir.join(format!(".cover-{key}.{}.part", Uuid::new_v4().simple()));
    if let Err(e) = tokio::fs::write(&temp, &bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Ok(ArtworkOutcome::Failed(format!(
            "the artwork cache can't be written: {e}"
        )));
    }
    if let Err(e) = tokio::fs::rename(&temp, &dest).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Ok(ArtworkOutcome::Failed(format!(
            "the artwork cache can't be written: {e}"
        )));
    }
    let dest_text = dest.to_string_lossy().to_string();
    // Recorded only while the feed still names the same address (a
    // check may have changed it meanwhile).
    let done =
        sqlx::query("UPDATE podcasts SET image_path = $2 WHERE id = $1 AND btrim(image_url) = $3")
            .bind(podcast_id)
            .bind(&dest_text)
            .bind(&image_url)
            .execute(pool)
            .await?;
    if done.rows_affected() == 0 {
        let _ = tokio::fs::remove_file(&dest).await;
        return Ok(ArtworkOutcome::Unchanged);
    }
    if let Some(old) = image_path.filter(|old| *old != dest_text) {
        let _ = tokio::fs::remove_file(old).await;
    }
    Ok(ArtworkOutcome::Cached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_recognised_by_their_first_bytes() {
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
        assert_eq!(sniff_image(b"RIFF\x10\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff_image(b"GIF89a\x01\0"), Some("gif"));
        assert_eq!(
            sniff_image(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(sniff_image(b"<!doctype html><title>404</title>"), None);
        assert_eq!(sniff_image(b""), None);
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WAVE"), None);
    }

    #[test]
    fn a_copy_is_named_by_a_hash_of_its_address() {
        let url = "https://cdn.example/art.jpg?token=fake-feed-token";
        let key = artwork_key(url);
        assert_eq!(key.len(), 16);
        assert_eq!(key, artwork_key(&format!("  {url} ")));
        assert_ne!(key, artwork_key("https://cdn.example/other.jpg"));
        assert!(!key.contains("token"));
        let path = format!("/cache/artwork/podcasts/x/cover-{key}.png");
        assert_eq!(key_of_path(&path), Some(key.as_str()));
        assert_eq!(key_of_path("/cache/poster-original.jpg"), None);
        assert_eq!(key_of_path("/cache/cover-short.jpg"), None);
        let id = Uuid::nil();
        assert_eq!(
            artwork_route(id, Some(&path)),
            Some(format!("/api/v1/podcasts/{id}/artwork?v={key}"))
        );
        assert_eq!(artwork_route(id, None), None);
    }

    #[test]
    fn only_kept_kinds_are_served() {
        assert_eq!(content_type_of("/a/cover-1.jpg"), Some("image/jpeg"));
        assert_eq!(content_type_of("/a/cover-1.webp"), Some("image/webp"));
        assert_eq!(content_type_of("/a/cover-1.svg"), None);
        assert_eq!(content_type_of("/a/cover-1"), None);
    }
}
