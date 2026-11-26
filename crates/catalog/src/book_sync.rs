//! The database half of AB6 (D30): writing a book's merged fields,
//! the `sidecar` slot of the merge, applying an admin's match (per
//! book, or a library-wide quick match), and the files Parlour may
//! write beside a book. Spec: `docs/AUDIOBOOKS-PODCASTS.md`,
//! "Metadata sources, matching and covers (AB6)".
//!
//! - **The server's copy.** A match leaves its result in
//!   `book_sidecars`: a `parlour-sidecar` document of everything the
//!   book then holds, kept in Parlour's own metadata area (D30's
//!   default). It fills the merge's `sidecar` slot, so a re-scan keeps
//!   the match, just as Audiobookshelf keeps a match in the
//!   `metadata.json` it saves in its metadata folder.
//! - **The file beside the book** (`book.parlour.json`) is written
//!   only where the library opted in (`libraries.write_sidecars`,
//!   XFER5), only in a folder that belongs to this book alone (never a
//!   library root, never a folder holding another book's audio), and
//!   atomically. A matched cover goes beside it as `cover.jpg` under
//!   the same rules, never over an image Parlour didn't write.
//! - **Which wins in the slot.** The book's own `book.parlour.json`
//!   when there is no server copy, or when the file changed since the
//!   match; otherwise the server copy. An admin's match beats the
//!   file until the file changes, as §13.6 of IMPORT-EXPORT.md has it
//!   for video.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

use parlour_metadata::books::{BookCandidate, BookProvider};

use crate::artwork::sha256_hex;
use crate::book_meta::{
    apply_match, book_sidecar_document, candidate_fields, sidecar_fields, BookFields, MatchMode,
};
use crate::sidecar::artwork::{decide, is_jpeg, CopyAction};
use crate::sidecar::{parse, to_bytes, write_atomic, Sidecar, WriteOutcome, BOOK_FILE_NAME};

// -------------------------------------------------------------- fields

/// A file's (path, size, mtime in µs) — how the slot tells whether a
/// book's `book.parlour.json` changed since a match. `None` when the
/// file isn't there.
pub fn file_fingerprint(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let mtime: chrono::DateTime<chrono::Utc> = meta.modified().ok()?.into();
    Some(format!(
        "{}\t{}\t{}",
        path.display(),
        meta.len(),
        mtime.timestamp_micros()
    ))
}

/// The `sidecar` slot: the book's own file, or the server's copy a
/// match left. The file wins when there is no copy, or when it
/// differs from the file the match saw (`recorded`).
pub fn choose_sidecar(
    file: Option<(BookFields, String)>,
    server: Option<(BookFields, Option<String>)>,
) -> Option<BookFields> {
    match (file, server) {
        (None, None) => None,
        (Some((f, _)), None) => Some(f),
        (None, Some((s, _))) => Some(s),
        (Some((f, fp)), Some((s, recorded))) => {
            if recorded.as_deref() == Some(fp.as_str()) {
                Some(s)
            } else {
                Some(f)
            }
        }
    }
}

/// The server's copy of a book's sidecar, and the file fingerprint the
/// match recorded. A stored document that no longer reads (it never
/// should) counts as none.
pub async fn server_copy(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Option<(BookFields, Option<String>)>, sqlx::Error> {
    let row: Option<(serde_json::Value, Option<String>)> =
        sqlx::query_as("SELECT document, file_fingerprint FROM book_sidecars WHERE item_id = $1")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(doc, fp)| {
        let bytes = serde_json::to_vec(&doc).ok()?;
        let doc: Sidecar = parse(&bytes).ok()?;
        Some((sidecar_fields(&doc), fp))
    }))
}

/// A book's fields as the database holds them now.
pub async fn load_fields(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Option<BookFields>, sqlx::Error> {
    type Row = (
        String,
        Option<i32>,
        Option<String>,
        Vec<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT i.title, i.year, i.overview, i.genres, b.subtitle, b.publisher, b.isbn,
                b.asin, b.language, b.google_books_id, b.open_library_id
         FROM media_items i LEFT JOIN book_metadata b ON b.item_id = i.id
         WHERE i.id = $1 AND i.kind = 'audiobook'",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((title, year, overview, genres, subtitle, publisher, isbn, asin, language, g, o)) =
        row
    else {
        return Ok(None);
    };
    let people: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.name, ip.role FROM item_people ip JOIN people p ON p.id = ip.person_id
         WHERE ip.item_id = $1 AND ip.role IN ('author', 'narrator')
         ORDER BY ip.role, ip.sort_order, p.name",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    let role = |r: &str| -> Vec<String> {
        people
            .iter()
            .filter(|(_, pr)| pr == r)
            .map(|(n, _)| n.clone())
            .collect()
    };
    Ok(Some(BookFields {
        title: Some(title),
        subtitle,
        authors: Some(role("author")),
        narrators: Some(role("narrator")),
        year,
        publisher,
        overview,
        genres: Some(genres),
        isbn,
        asin,
        language,
        google_books_id: g,
        open_library_id: o,
    }))
}

/// Replace a book's authors and narrators.
pub(crate) async fn write_people(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    f: &BookFields,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM item_people WHERE item_id = $1 AND role IN ('author', 'narrator')")
        .bind(item_id)
        .execute(pool)
        .await?;
    for (role, names) in [("author", &f.authors), ("narrator", &f.narrators)] {
        for (order, name) in names.iter().flatten().enumerate() {
            let (person_id,): (Uuid,) = sqlx::query_as(
                "INSERT INTO people (name) VALUES ($1)
                 ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
                 RETURNING id",
            )
            .bind(name)
            .fetch_one(pool)
            .await?;
            sqlx::query(
                "INSERT INTO item_people (item_id, person_id, role, sort_order)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (item_id, person_id, role) DO NOTHING",
            )
            .bind(item_id)
            .bind(person_id)
            .bind(role)
            .bind(order as i32)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Store a book's book-only fields. `fingerprint` replaces the stored
/// sources fingerprint when given and keeps it when `None`.
pub(crate) async fn write_details(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    f: &BookFields,
    fingerprint: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO book_metadata
             (item_id, subtitle, publisher, isbn, asin, language, google_books_id,
              open_library_id, sources_fingerprint, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
         ON CONFLICT (item_id) DO UPDATE SET
             subtitle = EXCLUDED.subtitle, publisher = EXCLUDED.publisher,
             isbn = EXCLUDED.isbn, asin = EXCLUDED.asin, language = EXCLUDED.language,
             google_books_id = EXCLUDED.google_books_id,
             open_library_id = EXCLUDED.open_library_id,
             sources_fingerprint = COALESCE(EXCLUDED.sources_fingerprint,
                                            book_metadata.sources_fingerprint),
             updated_at = now()",
    )
    .bind(item_id)
    .bind(&f.subtitle)
    .bind(&f.publisher)
    .bind(&f.isbn)
    .bind(&f.asin)
    .bind(&f.language)
    .bind(&f.google_books_id)
    .bind(&f.open_library_id)
    .bind(fingerprint)
    .execute(pool)
    .await?;
    Ok(())
}

/// Write a book's fields everywhere they live (outside a scan).
async fn write_fields(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    f: &BookFields,
) -> Result<(), sqlx::Error> {
    let title = f.title.clone().unwrap_or_default();
    sqlx::query(
        "UPDATE media_items SET title = $2, sort_title = $3, year = $4, overview = $5,
             genres = $6, updated_at = now()
         WHERE id = $1",
    )
    .bind(item_id)
    .bind(&title)
    .bind(crate::parse::sort_title(&title))
    .bind(f.year)
    .bind(&f.overview)
    .bind(f.genres.clone().unwrap_or_default())
    .execute(pool)
    .await?;
    write_people(pool, item_id, f).await?;
    write_details(pool, item_id, f, None).await
}

// --------------------------------------------------------- the folder

/// Where a book lives: its folder and whether Parlour may write there
/// (only when the library opted in).
struct BookFolder {
    library_id: Uuid,
    write_sidecars: bool,
    /// `None` for a book that is one file in a library root: nothing
    /// is ever written in a root (D14).
    folder: Option<PathBuf>,
}

async fn book_folder(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Option<BookFolder>, sqlx::Error> {
    let row: Option<(Uuid, String, bool)> = sqlx::query_as(
        "SELECT i.library_id, i.stable_key, l.write_sidecars
         FROM media_items i JOIN libraries l ON l.id = i.library_id
         WHERE i.id = $1 AND i.kind = 'audiobook'",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((library_id, stable_key, write_sidecars)) = row else {
        return Ok(None);
    };
    let key = stable_key
        .strip_prefix("audiobook:")
        .unwrap_or(&stable_key)
        .to_string();
    let file: Option<(String, String)> = sqlx::query_as(
        "SELECT path, relative_path FROM media_files
         WHERE item_id = $1 AND available ORDER BY track_index NULLS LAST, path LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let folder = file.and_then(|(abs, rel)| {
        if rel == key {
            return None; // one file in the root
        }
        let root = abs.strip_suffix(&rel)?.trim_end_matches('/');
        Some(PathBuf::from(format!("{root}/{key}")))
    });
    Ok(Some(BookFolder {
        library_id,
        write_sidecars,
        folder,
    }))
}

/// Whether Parlour may write in a book's folder: not a library root,
/// and no other book's available audio anywhere under it.
async fn folder_is_the_books(
    pool: &parlour_db::PgPool,
    library_id: Uuid,
    item_id: Uuid,
    folder: &Path,
) -> Result<bool, sqlx::Error> {
    let shown = folder.to_string_lossy().to_string();
    let roots: Vec<String> =
        sqlx::query_scalar("SELECT path FROM library_paths WHERE library_id = $1")
            .bind(library_id)
            .fetch_all(pool)
            .await?;
    if roots
        .iter()
        .any(|r| Path::new(r.trim_end_matches('/')) == folder)
    {
        return Ok(false);
    }
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM media_files
                         WHERE library_id = $1 AND available AND item_id <> $2
                           AND starts_with(path, $3))",
    )
    .bind(library_id)
    .bind(item_id)
    .bind(format!("{}/", shown.trim_end_matches('/')))
    .fetch_one(pool)
    .await?;
    Ok(!shared)
}

/// What writing beside one book did.
#[derive(Debug, Default, Clone, Serialize)]
pub struct BookWriteReport {
    /// `book.parlour.json` written (new or changed).
    pub sidecar_written: bool,
    /// `cover.jpg` written beside the book.
    pub cover_written: bool,
    /// `path: reason` for each file deliberately not written.
    pub left_alone: Vec<String>,
    /// `path: reason` for each file that couldn't be written.
    pub errors: Vec<String>,
}

/// Write `book.parlour.json` beside a book where its library opted
/// in, and return the file's fingerprint afterwards (also when it was
/// already up to date, or not written at all but present).
async fn write_book_sidecar(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    doc: &Sidecar,
    report: &mut BookWriteReport,
) -> Result<Option<String>, sqlx::Error> {
    let Some(place) = book_folder(pool, item_id).await? else {
        return Ok(None);
    };
    let Some(folder) = place.folder.clone() else {
        return Ok(None);
    };
    let path = folder.join(BOOK_FILE_NAME);
    if place.write_sidecars {
        if folder_is_the_books(pool, place.library_id, item_id, &folder).await? {
            if doc.problems().is_empty() {
                match write_atomic(&path, &to_bytes(doc)) {
                    Ok(WriteOutcome::Written) => report.sidecar_written = true,
                    Ok(WriteOutcome::Unchanged) => {}
                    Err(e) => report.errors.push(format!("{}: {e}", path.display())),
                }
            }
        } else {
            report.left_alone.push(format!(
                "{}: the folder isn't this book's alone (a library root, or another book's audio is under it)",
                path.display()
            ));
        }
    }
    Ok(file_fingerprint(&path))
}

/// Copy a matched cover beside its book as `cover.jpg`, where the
/// library opted in: JPEG only, only in the book's own folder, never
/// over a file Parlour didn't write, and never when the folder already
/// has its own cover image under another name.
async fn copy_cover_beside(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    bytes: &[u8],
    report: &mut BookWriteReport,
) -> Result<(), sqlx::Error> {
    let Some(place) = book_folder(pool, item_id).await? else {
        return Ok(());
    };
    let (true, Some(folder)) = (place.write_sidecars, place.folder) else {
        return Ok(());
    };
    let dest = folder.join("cover.jpg");
    let shown = dest.display().to_string();
    if !folder_is_the_books(pool, place.library_id, item_id, &folder).await? {
        report
            .left_alone
            .push(format!("{shown}: the folder isn't this book's alone"));
        return Ok(());
    }
    if !is_jpeg(bytes) {
        report
            .left_alone
            .push(format!("{shown}: the matched cover isn't a JPEG"));
        return Ok(());
    }
    if let Some(other) = crate::audiobooks::COVER_NAMES
        .iter()
        .filter(|n| **n != "cover.jpg")
        .find(|n| folder.join(n).exists())
    {
        report.left_alone.push(format!(
            "{shown}: the folder has its own cover image ({other})"
        ));
        return Ok(());
    }
    let existing = match std::fs::read(&dest) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            report.errors.push(format!("{shown}: {e}"));
            return Ok(());
        }
    };
    let recorded: Option<(String,)> =
        sqlx::query_as("SELECT sha256 FROM sidecar_artwork WHERE path = $1")
            .bind(&shown)
            .fetch_optional(pool)
            .await?;
    match decide(
        existing.as_deref(),
        recorded.as_ref().map(|(h,)| h.as_str()),
        bytes,
    ) {
        CopyAction::Unchanged => {}
        CopyAction::Foreign => report.left_alone.push(format!(
            "{shown}: a file Parlour didn't write is already there"
        )),
        CopyAction::Write | CopyAction::Replace => match write_atomic(&dest, bytes) {
            Ok(WriteOutcome::Unchanged) => {}
            Ok(WriteOutcome::Written) => {
                sqlx::query(
                    "INSERT INTO sidecar_artwork (path, item_id, kind, sha256)
                     VALUES ($1, $2, 'poster', $3)
                     ON CONFLICT (path) DO UPDATE SET
                        item_id = EXCLUDED.item_id, kind = EXCLUDED.kind,
                        sha256 = EXCLUDED.sha256, written_at = now()",
                )
                .bind(&shown)
                .bind(item_id)
                .bind(sha256_hex(bytes))
                .execute(pool)
                .await?;
                report.cover_written = true;
            }
            Err(e) => report.errors.push(format!("{shown}: {e}")),
        },
    }
    Ok(())
}

// --------------------------------------------------------------- covers

/// The image kind of some bytes, as a file extension: JPEG, PNG or
/// WebP. Anything else isn't taken as a cover.
pub fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if is_jpeg(bytes) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Write bytes into the artwork cache atomically (temp file + rename
/// in the same folder).
pub(crate) fn write_cache_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

/// Make a cached file the book's one poster, of a given variant
/// (`match`, `file` or `embedded`): every other poster row goes.
pub(crate) async fn set_poster(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
    variant: &str,
    path: &str,
    hash: Option<&str>,
    source_url: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM images WHERE item_id = $1 AND kind = 'poster' AND path <> $2")
        .bind(item_id)
        .bind(path)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO images (item_id, kind, variant, path, hash, source_url)
         VALUES ($1, 'poster', $2, $3, $4, $5)
         ON CONFLICT (item_id, kind, path) DO UPDATE SET
             variant = EXCLUDED.variant, hash = EXCLUDED.hash,
             source_url = EXCLUDED.source_url",
    )
    .bind(item_id)
    .bind(variant)
    .bind(path)
    .bind(hash)
    .bind(source_url)
    .execute(pool)
    .await?;
    Ok(())
}

async fn has_poster(pool: &parlour_db::PgPool, item_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM images WHERE item_id = $1 AND kind = 'poster'
                          AND variant <> 'generated' AND path <> ''
                          AND path NOT LIKE 'pending:%')",
    )
    .bind(item_id)
    .fetch_one(pool)
    .await
}

/// Download a matched cover into the artwork cache
/// (`<artwork>/<item>/poster-match.<ext>`) and make it the book's
/// poster. Returns the bytes.
async fn cache_match_cover(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    item_id: Uuid,
    url: &str,
) -> Result<Result<Vec<u8>, String>, sqlx::Error> {
    let bytes = match http.get(url).send().await {
        Ok(res) if res.status().is_success() => match res.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => return Ok(Err(format!("{url}: {e}"))),
        },
        Ok(res) => return Ok(Err(format!("{url}: HTTP {}", res.status().as_u16()))),
        Err(e) => return Ok(Err(format!("{url}: {e}"))),
    };
    let Some(ext) = image_extension(&bytes) else {
        return Ok(Err(format!("{url}: not a JPEG, PNG or WebP image")));
    };
    let dir = artwork_dir.join(item_id.to_string());
    let dest = dir.join(format!("poster-match.{ext}"));
    if let Err(e) = write_cache_file(&dest, &bytes) {
        return Ok(Err(format!("{}: {e}", dest.display())));
    }
    for other in ["jpg", "png", "webp"].iter().filter(|e| **e != ext) {
        let _ = std::fs::remove_file(dir.join(format!("poster-match.{other}")));
    }
    let path = dest.to_string_lossy().to_string();
    set_poster(
        pool,
        item_id,
        "match",
        &path,
        Some(&sha256_hex(&bytes)),
        Some(url),
    )
    .await?;
    Ok(Ok(bytes))
}

// ---------------------------------------------------------------- match

/// What one match did to one book.
#[derive(Debug, Default, Clone, Serialize)]
pub struct BookMatchReport {
    pub provider: String,
    pub provider_id: String,
    pub mode: String,
    /// Whether any of the book's values changed.
    pub changed: bool,
    /// `applied`, `kept` (the book keeps its cover: fill gaps and it
    /// has one, or the admin didn't ask), `none` (the provider has no
    /// cover) or `failed`.
    pub cover: String,
    #[serde(flatten)]
    pub written: BookWriteReport,
}

#[derive(Debug, thiserror::Error)]
pub enum BookMatchError {
    #[error("no such audiobook")]
    NotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Apply a provider's record to one book (D30): lay its values over
/// the book (`mode`), record the result as the server's copy of the
/// book's sidecar, write `book.parlour.json` beside the book where the
/// library opted in, and — when `use_cover` — take the provider's
/// cover (always with `Overwrite`; with `FillGaps` only when the book
/// has none). `record_unchanged` keeps a copy even when nothing
/// changed (an admin's explicit choice); a quick match records only
/// changes.
#[allow(clippy::too_many_arguments)]
pub async fn apply_book_match(
    pool: &parlour_db::PgPool,
    http: &reqwest::Client,
    artwork_dir: &Path,
    item_id: Uuid,
    candidate: &BookCandidate,
    mode: MatchMode,
    use_cover: bool,
    record_unchanged: bool,
) -> Result<BookMatchReport, BookMatchError> {
    let current = load_fields(pool, item_id)
        .await?
        .ok_or(BookMatchError::NotFound)?;
    let new = apply_match(&current, &candidate_fields(candidate), mode);
    let mut report = BookMatchReport {
        provider: candidate.provider.clone(),
        provider_id: candidate.provider_id.clone(),
        mode: mode.as_str().to_string(),
        changed: new != current,
        cover: "kept".to_string(),
        written: BookWriteReport::default(),
    };

    // The cover first, so `changed` counts it.
    let mut cover_bytes = None;
    if use_cover {
        match &candidate.cover_url {
            None => report.cover = "none".to_string(),
            Some(url) => {
                if mode == MatchMode::Overwrite || !has_poster(pool, item_id).await? {
                    match cache_match_cover(pool, http, artwork_dir, item_id, url).await? {
                        Ok(bytes) => {
                            report.cover = "applied".to_string();
                            report.changed = true;
                            cover_bytes = Some(bytes);
                        }
                        Err(e) => {
                            report.cover = "failed".to_string();
                            report.written.errors.push(e);
                        }
                    }
                }
            }
        }
    }

    if !report.changed && !record_unchanged {
        return Ok(report);
    }
    if new != current {
        write_fields(pool, item_id, &new).await?;
    }
    sqlx::query(
        "UPDATE media_items SET metadata_status = 'matched', metadata_provider = $2,
             last_metadata_at = now()
         WHERE id = $1",
    )
    .bind(item_id)
    .bind(&candidate.provider)
    .execute(pool)
    .await?;
    let doc = book_sidecar_document(&new);
    let file_fp = write_book_sidecar(pool, item_id, &doc, &mut report.written).await?;
    let document = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
    sqlx::query(
        "INSERT INTO book_sidecars
             (item_id, document, provider, provider_id, mode, file_fingerprint, matched_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())
         ON CONFLICT (item_id) DO UPDATE SET
             document = EXCLUDED.document, provider = EXCLUDED.provider,
             provider_id = EXCLUDED.provider_id, mode = EXCLUDED.mode,
             file_fingerprint = EXCLUDED.file_fingerprint, matched_at = now()",
    )
    .bind(item_id)
    .bind(document)
    .bind(&candidate.provider)
    .bind(&candidate.provider_id)
    .bind(mode.as_str())
    .bind(file_fp)
    .execute(pool)
    .await?;
    if let Some(bytes) = cover_bytes {
        copy_cover_beside(pool, item_id, &bytes, &mut report.written).await?;
    }
    Ok(report)
}

/// Forget a book's match: the server's copy and the matched cover go,
/// and the next scan merges the book from its local sources again. A
/// `book.parlour.json` or `cover.jpg` written beside the book stays
/// (Parlour never deletes beside the media); delete it by hand to drop
/// it too. Returns whether there was a match.
pub async fn forget_book_match(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let gone = sqlx::query("DELETE FROM book_sidecars WHERE item_id = $1")
        .bind(item_id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0;
    let covers: Vec<String> = sqlx::query_scalar(
        "DELETE FROM images WHERE item_id = $1 AND kind = 'poster' AND variant = 'match'
         RETURNING path",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    for path in covers {
        let _ = std::fs::remove_file(path);
    }
    sqlx::query("UPDATE book_metadata SET sources_fingerprint = NULL WHERE item_id = $1")
        .bind(item_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "UPDATE media_items SET metadata_status = 'unmatched', metadata_provider = NULL
         WHERE id = $1 AND kind = 'audiobook'",
    )
    .bind(item_id)
    .execute(pool)
    .await?;
    Ok(gone)
}

// ---------------------------------------------------------- quick match

/// A quick match takes a provider's result only when its title is at
/// least this close to the book's (`parlour_metadata::title_similarity`,
/// against the title and against "title subtitle").
pub const QUICK_MATCH_MIN_SIMILARITY: f64 = 0.5;

/// The first search result a quick match may take, or `None`.
pub fn quick_pick<'a>(book_title: &str, results: &'a [BookCandidate]) -> Option<&'a BookCandidate> {
    results.iter().find(|c| {
        let full = match &c.subtitle {
            Some(sub) => format!("{} {sub}", c.title),
            None => c.title.clone(),
        };
        parlour_metadata::title_similarity(book_title, &c.title)
            .max(parlour_metadata::title_similarity(book_title, &full))
            >= QUICK_MATCH_MIN_SIMILARITY
    })
}

/// Quick-match every available book of an audiobooks library against
/// one provider: search by title (and first author), take the first
/// close result, and **fill gaps** only — Audiobookshelf's default for
/// its library-wide quick match. Covers are filled too, only where a
/// book has none. One book's failure is listed on the job and the rest
/// carry on. `pace` is waited between books, to stay inside the
/// providers' anonymous rate limits.
pub async fn quick_match_library(
    pool: &parlour_db::PgPool,
    provider: &BookProvider,
    http: &reqwest::Client,
    artwork_dir: &Path,
    library_id: Uuid,
    job_id: Option<Uuid>,
    pace: Duration,
) -> Result<serde_json::Value, sqlx::Error> {
    let books: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT i.id, i.title,
                (SELECT p.name FROM item_people ip JOIN people p ON p.id = ip.person_id
                  WHERE ip.item_id = i.id AND ip.role = 'author'
                  ORDER BY ip.sort_order LIMIT 1)
         FROM media_items i
         WHERE i.library_id = $1 AND i.kind = 'audiobook' AND i.available
         ORDER BY i.sort_title, i.id",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;
    let total = books.len() as i32;
    if let Some(job) = job_id {
        let _ = parlour_jobs::set_progress(pool, job, 0, total).await;
    }
    let (mut matched, mut unchanged, mut no_match, mut failed) = (0, 0, 0, 0);
    let (mut sidecars_written, mut covers_applied) = (0, 0);
    let mut left_alone: Vec<String> = Vec::new();
    for (done, (item_id, title, author)) in books.iter().enumerate() {
        if done > 0 && !pace.is_zero() {
            tokio::time::sleep(pace).await;
        }
        let outcome = async {
            let results = provider
                .search(title, author.as_deref())
                .await
                .map_err(|e| format!("{title}: {e}"))?;
            let Some(pick) = quick_pick(title, &results) else {
                return Ok(None);
            };
            // A search result carries only a small thumbnail; fetch
            // the full record when its cover may be used.
            let mut pick = pick.clone();
            if provider.name() == "google"
                && !has_poster(pool, *item_id)
                    .await
                    .map_err(|e| e.to_string())?
            {
                if let Ok(full) = provider.get_book(&pick.provider_id).await {
                    pick = full;
                }
            }
            apply_book_match(
                pool,
                http,
                artwork_dir,
                *item_id,
                &pick,
                MatchMode::FillGaps,
                true,
                false,
            )
            .await
            .map(Some)
            .map_err(|e| format!("{title}: {e}"))
        }
        .await;
        match outcome {
            Ok(None) => no_match += 1,
            Ok(Some(r)) => {
                if r.changed {
                    matched += 1;
                } else {
                    unchanged += 1;
                }
                sidecars_written += r.written.sidecar_written as i32;
                covers_applied += (r.cover == "applied") as i32;
                for e in &r.written.errors {
                    if let Some(job) = job_id {
                        let _ = parlour_jobs::add_error(pool, job, None, e).await;
                    }
                }
                for note in r.written.left_alone {
                    if left_alone.len() < 100 {
                        left_alone.push(note);
                    }
                }
            }
            Err(e) => {
                failed += 1;
                if let Some(job) = job_id {
                    let _ = parlour_jobs::add_error(pool, job, None, &e).await;
                }
            }
        }
        if let Some(job) = job_id {
            let _ = parlour_jobs::set_progress(pool, job, done as i32 + 1, total).await;
        }
    }
    Ok(serde_json::json!({
        "quick_match": true,
        "provider": provider.name(),
        "mode": MatchMode::FillGaps.as_str(),
        "total": total,
        "matched": matched,
        "unchanged": unchanged,
        "no_match": no_match,
        "failed": failed,
        "covers_applied": covers_applied,
        "sidecars_written": sidecars_written,
        "left_alone": left_alone,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(title: &str) -> BookFields {
        BookFields {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_slot_prefers_the_match_until_the_file_changes() {
        let file = || Some((named("file"), "fp-now".to_string()));
        assert_eq!(choose_sidecar(None, None), None);
        assert_eq!(choose_sidecar(file(), None), Some(named("file")));
        assert_eq!(
            choose_sidecar(None, Some((named("server"), None))),
            Some(named("server"))
        );
        // The file the match saw (or wrote) is still there: the match.
        assert_eq!(
            choose_sidecar(file(), Some((named("server"), Some("fp-now".into())))),
            Some(named("server"))
        );
        // Changed since, or new since: the file.
        assert_eq!(
            choose_sidecar(file(), Some((named("server"), Some("fp-then".into())))),
            Some(named("file"))
        );
        assert_eq!(
            choose_sidecar(file(), Some((named("server"), None))),
            Some(named("file"))
        );
    }

    #[test]
    fn quick_match_takes_the_first_close_result() {
        let c = |id: &str, title: &str, sub: Option<&str>| BookCandidate {
            provider: "google".into(),
            provider_id: id.into(),
            title: title.into(),
            subtitle: sub.map(str::to_string),
            ..Default::default()
        };
        let results = vec![
            c("a", "Completely Different", None),
            c("b", "The Lantern Keeper", Some("A Harbour Tale")),
            c("c", "The Lantern Keeper", None),
        ];
        assert_eq!(
            quick_pick("The Lantern Keeper", &results).map(|c| c.provider_id.as_str()),
            Some("b")
        );
        assert_eq!(
            quick_pick("The Lantern Keeper: A Harbour Tale", &results[1..2])
                .map(|c| c.provider_id.as_str()),
            Some("b"),
            "a title folder carrying the subtitle still matches"
        );
        assert!(quick_pick("Nothing Alike", &results[..1]).is_none());
        assert!(quick_pick("x", &[]).is_none());
    }

    #[test]
    fn covers_are_sniffed_not_trusted() {
        assert_eq!(image_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(image_extension(b"<html>"), None);
        let dir = std::env::temp_dir().join(format!("parlour-ab6-cache-{}", Uuid::new_v4()));
        let dest = dir.join("x").join("poster-match.jpg");
        write_cache_file(&dest, b"one").unwrap();
        write_cache_file(&dest, b"two").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"two");
        assert_eq!(
            std::fs::read_dir(dest.parent().unwrap()).unwrap().count(),
            1
        );
        assert!(file_fingerprint(&dest).unwrap().contains("\t3\t"));
        assert!(file_fingerprint(&dir.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
