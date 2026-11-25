//! XFER2: export the whole server as a `parlour-export` v1 bundle
//! (D14, D25; spec `docs/IMPORT-EXPORT.md`, §11 for this task).
//!
//! - `GET /api/v1/admin/export/estimate`: what opting into artwork
//!   would add (files and bytes), so a client can warn about the
//!   size before asking for it.
//! - `POST /api/v1/admin/export`: queue an export job. Options are
//!   `include_artwork` and `include_hashes`, both **off by default**
//!   (an empty body means both off). `include_hashes` is the
//!   explicit per-export confirm D14 asks for: without it no
//!   password or PIN hash leaves the server.
//! - `GET /api/v1/admin/export/{job}/download`: the finished bundle.
//! - `DELETE /api/v1/admin/export/{job}`: remove a bundle from the
//!   server's disk once it has been fetched.
//!
//! The job reads every carried column (`bundle::coverage`) in one
//! `REPEATABLE READ` transaction, so the references between the
//! documents agree; resolves database ids to names; writes the
//! documents in a fixed order, so two exports of an unchanged server
//! differ only in their export time; and stores the zip under the
//! cache directory (`<PARLOUR_CACHE_DIR>/exports/<job id>.zip`). The
//! job's result carries the counts and the download's file name, and
//! the Jobs page offers the download. The request runs the job at
//! once; [`spawn_worker`] picks up any export left queued (a restart
//! between the queueing and the start).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::bundle::{
    artwork_file_name, AccessGrant, ActivityEntry, Bundle, BundleWriter, CatalogFile, Credit,
    Image, ImageKind, Item, ItemKind, ItemRef, ItemUserData, LibrariesFile, Library, LibraryKind,
    Manifest, MetadataStatus, Preferences, Profile, ProfileRef, ProfileUserData, ProfilesFile,
    RecentSearch, Segment, SegmentKind, SegmentSkip, SegmentSource, SubtitleMode, User,
    UserDataFile, UsersFile,
};
use crate::error::AppError;
use crate::http::AppState;

/// What one export includes beyond the always-carried documents.
/// Both are off unless the request turns them on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportOptions {
    /// The admin's explicit confirm that password and PIN hashes may
    /// leave the server in this one bundle (D14; D25 lets them sit in
    /// the plain zip). Off by default.
    #[serde(default)]
    pub include_hashes: bool,
    /// Put the cached provider artwork in the zip under `artwork/`.
    /// Off by default: it can make the bundle many times larger.
    #[serde(default)]
    pub include_artwork: bool,
}

impl ExportOptions {
    /// The options a queued job carries. A missing or non-boolean
    /// flag reads as off, so a damaged payload can never turn hashes
    /// on.
    fn from_payload(payload: &serde_json::Value) -> Self {
        Self {
            include_hashes: payload["include_hashes"].as_bool().unwrap_or(false),
            include_artwork: payload["include_artwork"].as_bool().unwrap_or(false),
        }
    }
}

// ------------------------------------------------------------ the snapshot

#[derive(Debug, Clone, sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    username: String,
    password_hash: String,
    is_admin: bool,
    created_at: DateTime<Utc>,
    disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProfileRow {
    id: Uuid,
    user_id: Uuid,
    name: String,
    pin_hash: Option<String>,
    avatar_color: Option<String>,
    audio_lang: Option<String>,
    subtitle_lang: Option<String>,
    subtitle_mode: String,
    remote_bitrate_cap_kbps: Option<i32>,
    created_at: DateTime<Utc>,
    home_rail_order: Vec<String>,
    home_rails_hidden: Vec<String>,
    segment_skip: String,
    autoplay_next: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RecentSearchRow {
    profile_id: Uuid,
    query: String,
    searched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LibraryRow {
    id: Uuid,
    name: String,
    kind: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LibraryPathRow {
    library_id: Uuid,
    path: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AccessRow {
    library_id: Uuid,
    profile_id: Uuid,
    max_rating: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ItemRow {
    id: Uuid,
    library_id: Uuid,
    parent_id: Option<Uuid>,
    kind: String,
    stable_key: String,
    title: String,
    sort_title: String,
    year: Option<i32>,
    season_number: Option<i32>,
    episode_number: Option<i32>,
    overview: Option<String>,
    content_rating: Option<String>,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    genres: Vec<String>,
    vote_average: Option<f64>,
    runtime_minutes: Option<i32>,
    air_date: Option<NaiveDate>,
    metadata_status: String,
    metadata_provider: Option<String>,
    last_metadata_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CreditRow {
    item_id: Uuid,
    name: String,
    tmdb_id: Option<i64>,
    role: String,
    character_name: Option<String>,
    sort_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SegmentRow {
    item_id: Uuid,
    kind: String,
    source: String,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    confidence: f32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// An `images` row that has a `source_url` (provider artwork); the
/// others (generated tiles, audiobook folder covers) stay behind.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ImageRow {
    item_id: Uuid,
    kind: String,
    variant: String,
    source_url: String,
    path: String,
    hash: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct UserDataRow {
    profile_id: Uuid,
    item_id: Uuid,
    position_ms: i64,
    duration_ms: Option<i64>,
    played: bool,
    is_favorite: bool,
    play_count: i32,
    last_played_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ActivityRow {
    id: Uuid,
    created_at: DateTime<Utc>,
    kind: String,
    user_id: Option<Uuid>,
    profile_id: Option<Uuid>,
    item_id: Option<Uuid>,
    actor: Option<String>,
    summary: String,
    detail: serde_json::Value,
}

/// Every carried row, read at one instant.
#[derive(Debug, Clone)]
struct Snapshot {
    /// The transaction's `now()`: the manifest's `exported_at`.
    taken_at: DateTime<Utc>,
    users: Vec<UserRow>,
    profiles: Vec<ProfileRow>,
    recent_searches: Vec<RecentSearchRow>,
    libraries: Vec<LibraryRow>,
    paths: Vec<LibraryPathRow>,
    access: Vec<AccessRow>,
    items: Vec<ItemRow>,
    credits: Vec<CreditRow>,
    segments: Vec<SegmentRow>,
    images: Vec<ImageRow>,
    user_data: Vec<UserDataRow>,
    activity: Vec<ActivityRow>,
}

/// Read every `Value` column of `bundle::coverage::SCHEMA`, plus the
/// ids its `Reference` columns resolve through, in one read-only
/// `REPEATABLE READ` transaction: every query sees the same instant,
/// so a profile, item or user named anywhere is there everywhere.
async fn read_snapshot(pool: &parlour_db::PgPool) -> Result<Snapshot, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let (taken_at,): (DateTime<Utc>,) = sqlx::query_as("SELECT now()").fetch_one(&mut *tx).await?;
    let users = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, is_admin, created_at, disabled_at FROM users",
    )
    .fetch_all(&mut *tx)
    .await?;
    let profiles = sqlx::query_as::<_, ProfileRow>(
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang,
                subtitle_mode, remote_bitrate_cap_kbps, created_at, home_rail_order,
                home_rails_hidden, segment_skip, autoplay_next
         FROM profiles",
    )
    .fetch_all(&mut *tx)
    .await?;
    let recent_searches = sqlx::query_as::<_, RecentSearchRow>(
        "SELECT profile_id, query, searched_at FROM recent_searches",
    )
    .fetch_all(&mut *tx)
    .await?;
    let libraries =
        sqlx::query_as::<_, LibraryRow>("SELECT id, name, kind, created_at FROM libraries")
            .fetch_all(&mut *tx)
            .await?;
    let paths = sqlx::query_as::<_, LibraryPathRow>("SELECT library_id, path FROM library_paths")
        .fetch_all(&mut *tx)
        .await?;
    let access = sqlx::query_as::<_, AccessRow>(
        "SELECT library_id, profile_id, max_rating FROM library_access",
    )
    .fetch_all(&mut *tx)
    .await?;
    let items = sqlx::query_as::<_, ItemRow>(
        "SELECT id, library_id, parent_id, kind, stable_key, title, sort_title, year,
                season_number, episode_number, overview, content_rating, tmdb_id, tvdb_id,
                imdb_id, created_at, updated_at, genres, vote_average, runtime_minutes,
                air_date, metadata_status, metadata_provider, last_metadata_at
         FROM media_items",
    )
    .fetch_all(&mut *tx)
    .await?;
    let credits = sqlx::query_as::<_, CreditRow>(
        "SELECT ip.item_id, p.name, p.tmdb_id, ip.role, ip.character_name, ip.sort_order
         FROM item_people ip JOIN people p ON p.id = ip.person_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let segments = sqlx::query_as::<_, SegmentRow>(
        "SELECT item_id, kind, source, start_ms, end_ms, confidence, created_at, updated_at
         FROM media_segments",
    )
    .fetch_all(&mut *tx)
    .await?;
    let images = sqlx::query_as::<_, ImageRow>(
        "SELECT item_id, kind, variant, source_url, path, hash
         FROM images WHERE source_url IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?;
    let user_data = sqlx::query_as::<_, UserDataRow>(
        "SELECT profile_id, item_id, position_ms, duration_ms, played, is_favorite,
                play_count, last_played_at, created_at, updated_at
         FROM user_data",
    )
    .fetch_all(&mut *tx)
    .await?;
    let activity = sqlx::query_as::<_, ActivityRow>(
        "SELECT id, created_at, kind, user_id, profile_id, item_id, actor, summary, detail
         FROM activity",
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Snapshot {
        taken_at,
        users,
        profiles,
        recent_searches,
        libraries,
        paths,
        access,
        items,
        credits,
        segments,
        images,
        user_data,
        activity,
    })
}

// ------------------------------------------------------------ artwork

/// The cached file behind a provider image, keyed the way the bundle
/// names it: (`images.path`, `images.hash`).
type ArtworkKey = (String, String);

/// Whether an image row points at a file in the artwork cache. Only
/// those can travel as a `file` (spec §11): rows still waiting for
/// their download have an empty or `pending:` path, or no hash.
fn cached_key(image: &ImageRow) -> Option<ArtworkKey> {
    let hash = image.hash.as_deref()?;
    let real = !image.path.is_empty() && !image.path.starts_with("pending:");
    real.then(|| (image.path.clone(), hash.to_string()))
}

/// The bundle name of a cached file: `artwork/<hash>.<extension of
/// the cached path>`, the extension lowercased. `None` when the hash
/// or the extension doesn't fit the format's strict pattern.
fn artwork_name(path: &str, hash: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    artwork_file_name(hash, &ext)
}

/// Check a cached file before it is promised to the bundle: it must
/// exist and its bytes must still hash to `images.hash`. Returns the
/// file's size. (The writer checks the hash again as it copies.)
fn verify_artwork(path: &Path, sha256: &str) -> Result<u64, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("the cached file can't be read: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("the cached file can't be read: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != sha256 {
        return Err("the cached file has changed since it was downloaded".into());
    }
    Ok(size)
}

/// Keep one provider image per (item, kind, variant), the bundle's
/// key for an image. The database keys images by (item, kind, path),
/// so a re-match can leave a stale row beside a cached one (an
/// episode's still is re-inserted as `pending:still:original` next to
/// the downloaded file). The cached row wins; otherwise the lowest
/// path. Returns the kept rows and the ones left out.
fn one_image_per_key(images: Vec<ImageRow>) -> (Vec<ImageRow>, Vec<ImageRow>) {
    let mut groups: BTreeMap<(Uuid, String, String), Vec<ImageRow>> = BTreeMap::new();
    for image in images {
        groups
            .entry((image.item_id, image.kind.clone(), image.variant.clone()))
            .or_default()
            .push(image);
    }
    let mut kept = Vec::with_capacity(groups.len());
    let mut dropped = Vec::new();
    for (_, mut rows) in groups {
        rows.sort_by(|a, b| {
            cached_key(b)
                .is_some()
                .cmp(&cached_key(a).is_some())
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.source_url.cmp(&b.source_url))
        });
        let mut rows = rows.into_iter();
        kept.extend(rows.next());
        dropped.extend(rows);
    }
    (kept, dropped)
}

// ------------------------------------------------------------ building

/// Turn database rows into the six documents: ids become names, and
/// every list is put in the spec's fixed order (§11), comparing names
/// byte by byte, so the output never depends on the order Postgres
/// returned the rows in. `artwork` holds the cached files that were
/// checked and may be named (empty unless the export opted in).
fn build(
    snapshot: &Snapshot,
    options: ExportOptions,
    artwork: &HashMap<ArtworkKey, String>,
) -> Result<Bundle, String> {
    let usernames: HashMap<Uuid, &str> = snapshot
        .users
        .iter()
        .map(|u| (u.id, u.username.as_str()))
        .collect();
    let username = |id: Uuid| -> Result<String, String> {
        usernames
            .get(&id)
            .map(|name| name.to_string())
            .ok_or_else(|| format!("user {id} is referenced but missing"))
    };
    let mut profile_refs: HashMap<Uuid, ProfileRef> = HashMap::new();
    for profile in &snapshot.profiles {
        profile_refs.insert(
            profile.id,
            ProfileRef {
                username: username(profile.user_id)?,
                profile: profile.name.clone(),
            },
        );
    }
    let profile_ref = |id: Uuid| -> Result<ProfileRef, String> {
        profile_refs
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("profile {id} is referenced but missing"))
    };
    let library_names: HashMap<Uuid, &str> = snapshot
        .libraries
        .iter()
        .map(|l| (l.id, l.name.as_str()))
        .collect();
    let library_name = |id: Uuid| -> Result<String, String> {
        library_names
            .get(&id)
            .map(|name| name.to_string())
            .ok_or_else(|| format!("library {id} is referenced but missing"))
    };
    let mut item_refs: HashMap<Uuid, ItemRef> = HashMap::new();
    for item in &snapshot.items {
        item_refs.insert(
            item.id,
            ItemRef {
                library: library_name(item.library_id)?,
                stable_key: item.stable_key.clone(),
            },
        );
    }
    let item_ref = |id: Uuid| -> Result<ItemRef, String> {
        item_refs
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("item {id} is referenced but missing"))
    };

    // users.json — PIN hashes sit with their user, by profile name.
    let mut pins: HashMap<Uuid, BTreeMap<String, String>> = HashMap::new();
    for profile in &snapshot.profiles {
        let entry = pins.entry(profile.user_id).or_default();
        if let Some(hash) = &profile.pin_hash {
            entry.insert(profile.name.clone(), hash.clone());
        }
    }
    let mut users: Vec<User> = snapshot
        .users
        .iter()
        .map(|u| User {
            username: u.username.clone(),
            is_admin: u.is_admin,
            created_at: u.created_at,
            disabled_at: u.disabled_at,
            password_hash: options.include_hashes.then(|| u.password_hash.clone()),
            pin_hashes: options
                .include_hashes
                .then(|| pins.get(&u.id).cloned().unwrap_or_default()),
        })
        .collect();
    users.sort_by(|a, b| a.username.cmp(&b.username));

    // profiles.json — recent searches newest first.
    let mut searches: HashMap<Uuid, Vec<RecentSearch>> = HashMap::new();
    for search in &snapshot.recent_searches {
        searches
            .entry(search.profile_id)
            .or_default()
            .push(RecentSearch {
                query: search.query.clone(),
                searched_at: search.searched_at,
            });
    }
    let mut profiles = Vec::with_capacity(snapshot.profiles.len());
    for p in &snapshot.profiles {
        let mut recent = searches.remove(&p.id).unwrap_or_default();
        recent.sort_by(|a, b| {
            b.searched_at
                .cmp(&a.searched_at)
                .then_with(|| a.query.cmp(&b.query))
        });
        profiles.push(Profile {
            username: username(p.user_id)?,
            name: p.name.clone(),
            created_at: p.created_at,
            has_pin: p.pin_hash.is_some(),
            avatar_color: p.avatar_color.clone(),
            preferences: Preferences {
                audio_lang: p.audio_lang.clone(),
                subtitle_lang: p.subtitle_lang.clone(),
                subtitle_mode: closed(SubtitleMode::parse, "subtitle_mode", &p.subtitle_mode)?,
                remote_bitrate_cap_kbps: p.remote_bitrate_cap_kbps,
                home_rail_order: p.home_rail_order.clone(),
                home_rails_hidden: p.home_rails_hidden.clone(),
                segment_skip: closed(SegmentSkip::parse, "segment_skip", &p.segment_skip)?,
                autoplay_next: p.autoplay_next,
            },
            recent_searches: recent,
        });
    }
    profiles.sort_by(|a, b| {
        a.username
            .cmp(&b.username)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.name.cmp(&b.name))
    });

    // libraries.json — paths and grants under their library.
    let mut paths: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in &snapshot.paths {
        paths
            .entry(row.library_id)
            .or_default()
            .push(row.path.clone());
    }
    let mut grants: HashMap<Uuid, Vec<AccessGrant>> = HashMap::new();
    for row in &snapshot.access {
        let profile = profile_ref(row.profile_id)?;
        grants.entry(row.library_id).or_default().push(AccessGrant {
            username: profile.username,
            profile: profile.profile,
            max_rating: row.max_rating.clone(),
        });
    }
    let mut libraries = Vec::with_capacity(snapshot.libraries.len());
    for l in &snapshot.libraries {
        let mut lib_paths = paths.remove(&l.id).unwrap_or_default();
        lib_paths.sort();
        let mut access = grants.remove(&l.id).unwrap_or_default();
        access.sort_by(|a, b| {
            a.username
                .cmp(&b.username)
                .then_with(|| a.profile.cmp(&b.profile))
        });
        libraries.push(Library {
            name: l.name.clone(),
            kind: closed(LibraryKind::parse, "library kind", &l.kind)?,
            created_at: l.created_at,
            paths: lib_paths,
            access,
        });
    }
    libraries.sort_by(|a, b| a.name.cmp(&b.name));

    // catalog.json — people, segments and provider artwork per item.
    let mut credits: HashMap<Uuid, Vec<Credit>> = HashMap::new();
    for row in &snapshot.credits {
        credits.entry(row.item_id).or_default().push(Credit {
            name: row.name.clone(),
            tmdb_id: row.tmdb_id,
            role: row.role.clone(),
            character_name: row.character_name.clone(),
            sort_order: row.sort_order,
        });
    }
    let mut segments: HashMap<Uuid, Vec<Segment>> = HashMap::new();
    for row in &snapshot.segments {
        segments.entry(row.item_id).or_default().push(Segment {
            kind: closed(SegmentKind::parse, "segment kind", &row.kind)?,
            source: closed(SegmentSource::parse, "segment source", &row.source)?,
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            confidence: row.confidence,
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    let mut images: HashMap<Uuid, Vec<Image>> = HashMap::new();
    for row in &snapshot.images {
        let file = if options.include_artwork {
            cached_key(row).and_then(|key| artwork.get(&key).cloned())
        } else {
            None
        };
        images.entry(row.item_id).or_default().push(Image {
            kind: closed(ImageKind::parse, "image kind", &row.kind)?,
            variant: row.variant.clone(),
            source_url: row.source_url.clone(),
            file,
        });
    }
    let parents: HashMap<Uuid, &str> = snapshot
        .items
        .iter()
        .map(|i| (i.id, i.stable_key.as_str()))
        .collect();
    let mut items = Vec::with_capacity(snapshot.items.len());
    for i in &snapshot.items {
        let parent = match i.parent_id {
            Some(id) => Some(
                parents
                    .get(&id)
                    .map(|key| key.to_string())
                    .ok_or_else(|| format!("item {id} is referenced but missing"))?,
            ),
            None => None,
        };
        let mut people = credits.remove(&i.id).unwrap_or_default();
        people.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.role.cmp(&b.role))
        });
        let mut item_segments = segments.remove(&i.id).unwrap_or_default();
        // Kinds and sources in the order the format declares them.
        item_segments.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.source.cmp(&b.source)));
        let mut item_images = images.remove(&i.id).unwrap_or_default();
        item_images.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.variant.cmp(&b.variant)));
        items.push(Item {
            library: library_name(i.library_id)?,
            stable_key: i.stable_key.clone(),
            parent,
            kind: closed(ItemKind::parse, "item kind", &i.kind)?,
            title: i.title.clone(),
            sort_title: i.sort_title.clone(),
            year: i.year,
            season_number: i.season_number,
            episode_number: i.episode_number,
            air_date: i.air_date,
            overview: i.overview.clone(),
            genres: i.genres.clone(),
            runtime_minutes: i.runtime_minutes,
            content_rating: i.content_rating.clone(),
            vote_average: i.vote_average,
            tmdb_id: i.tmdb_id,
            tvdb_id: i.tvdb_id,
            imdb_id: i.imdb_id.clone(),
            metadata_status: closed(MetadataStatus::parse, "metadata_status", &i.metadata_status)?,
            metadata_provider: i.metadata_provider.clone(),
            last_metadata_at: i.last_metadata_at,
            created_at: i.created_at,
            updated_at: i.updated_at,
            people,
            segments: item_segments,
            images: item_images,
        });
    }
    items.sort_by(|a, b| {
        a.library
            .cmp(&b.library)
            .then_with(|| a.stable_key.cmp(&b.stable_key))
    });

    // user_data.json — one group per profile with watch state.
    let mut watch: BTreeMap<ProfileRef, Vec<ItemUserData>> = BTreeMap::new();
    for row in &snapshot.user_data {
        watch
            .entry(profile_ref(row.profile_id)?)
            .or_default()
            .push(ItemUserData {
                item: item_ref(row.item_id)?,
                position_ms: row.position_ms,
                duration_ms: row.duration_ms,
                played: row.played,
                is_favorite: row.is_favorite,
                play_count: row.play_count,
                last_played_at: row.last_played_at,
                created_at: row.created_at,
                updated_at: row.updated_at,
            });
    }
    let watch = watch
        .into_iter()
        .map(|(profile, mut rows)| {
            rows.sort_by(|a, b| a.item.cmp(&b.item));
            ProfileUserData {
                username: profile.username,
                profile: profile.profile,
                items: rows,
            }
        })
        .collect();

    // The Activity feed, oldest first; the row id only breaks ties
    // between rows written in the same microsecond.
    let mut rows: Vec<&ActivityRow> = snapshot.activity.iter().collect();
    rows.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut activity = Vec::with_capacity(rows.len());
    for row in rows {
        activity.push(ActivityEntry {
            created_at: row.created_at,
            kind: row.kind.clone(),
            username: row.user_id.map(username).transpose()?,
            profile: row.profile_id.map(profile_ref).transpose()?,
            item: row.item_id.map(item_ref).transpose()?,
            actor: row.actor.clone(),
            summary: row.summary.clone(),
            detail: row.detail.clone(),
        });
    }

    Ok(Bundle {
        manifest: Manifest::new(
            snapshot.taken_at,
            options.include_hashes,
            options.include_artwork,
        ),
        users: UsersFile { users },
        profiles: ProfilesFile { profiles },
        libraries: LibrariesFile { libraries },
        catalog: CatalogFile { items },
        user_data: UserDataFile {
            profiles: watch,
            activity,
        },
    })
}

/// A closed value set's member, or an error naming what the database
/// held that the format doesn't know.
fn closed<T>(parse: fn(&str) -> Option<T>, what: &str, value: &str) -> Result<T, String> {
    parse(value)
        .ok_or_else(|| format!("the database holds a {what} the format doesn't know: '{value}'"))
}

// ------------------------------------------------------------ writing

/// Where a job's bundle lives on the server.
fn bundle_path(export_dir: &Path, job_id: Uuid) -> PathBuf {
    export_dir.join(format!("{job_id}.zip"))
}

/// The name a download is offered under, from the export time.
fn download_name(exported_at: DateTime<Utc>) -> String {
    format!("parlour-export-{}.zip", exported_at.format("%Y%m%d-%H%M%S"))
}

/// Write the zip next to `dest` and move it into place only once it
/// is complete, so a half-written bundle is never offered. `files`
/// maps each artwork name the catalog uses to a cached file. Returns
/// the bundle's size in bytes.
fn write_bundle(
    dest: &Path,
    bundle: &Bundle,
    files: &BTreeMap<String, PathBuf>,
) -> Result<u64, String> {
    let part = dest.with_extension("zip.part");
    let written = (|| -> Result<u64, String> {
        let file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut writer =
            BundleWriter::new(BufWriter::new(file), bundle).map_err(|e| e.to_string())?;
        for (name, path) in files {
            let mut cached = std::fs::File::open(path)
                .map_err(|e| format!("{name}: the cached file can't be read: {e}"))?;
            writer
                .add_artwork(name, &mut cached)
                .map_err(|e| e.to_string())?;
        }
        let mut out = writer.finish().map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        let file = out.into_inner().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
        Ok(size)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// What a finished export reports on its job (the Jobs page lists
/// these, and the download uses `file_name`).
#[derive(Debug, Clone, Serialize)]
struct ExportSummary {
    file_name: String,
    bytes: u64,
    hashes_included: bool,
    artwork_included: bool,
    users: usize,
    profiles: usize,
    libraries: usize,
    items: usize,
    watch_state: usize,
    recent_searches: usize,
    activity: usize,
    artwork_files: usize,
    /// Cached images that couldn't go in (missing or changed file,
    /// or a name the format refuses); each is listed on the job's
    /// errors and travels as a provider reference only.
    artwork_left_out: usize,
    /// Provider image rows left out because another row already holds
    /// their (item, kind, variant) — see [`one_image_per_key`].
    duplicate_images_left_out: usize,
}

/// One whole export: snapshot, artwork checks, documents, zip.
async fn export(
    pool: &parlour_db::PgPool,
    job_id: Uuid,
    options: ExportOptions,
    export_dir: &Path,
) -> Result<ExportSummary, String> {
    let mut snapshot = read_snapshot(pool)
        .await
        .map_err(|e| format!("reading the database: {e}"))?;
    let (images, duplicates) = one_image_per_key(std::mem::take(&mut snapshot.images));
    snapshot.images = images;
    for image in &duplicates {
        let _ = parlour_jobs::add_error(
            pool,
            job_id,
            Some(&image.path),
            &format!(
                "a second {} image '{}' for the same item was left out (the bundle keeps one per kind and variant)",
                image.kind, image.variant
            ),
        )
        .await;
    }

    // Artwork: check each distinct cached file once.
    let mut artwork: HashMap<ArtworkKey, String> = HashMap::new();
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut left_out = 0usize;
    if options.include_artwork {
        let keys: BTreeSet<ArtworkKey> = snapshot.images.iter().filter_map(cached_key).collect();
        let total = i32::try_from(keys.len() + 1).unwrap_or(i32::MAX);
        let _ = parlour_jobs::set_progress(pool, job_id, 0, total).await;
        for (done, (path, hash)) in keys.into_iter().enumerate() {
            let checked = match artwork_name(&path, &hash) {
                None => Err("its hash or file extension can't be named in a bundle".to_string()),
                // Another cached copy of the same bytes is already in.
                Some(name) if files.contains_key(&name) => Ok(name),
                Some(name) => {
                    let file = PathBuf::from(&path);
                    let sha = hash.clone();
                    tokio::task::spawn_blocking(move || verify_artwork(&file, &sha))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                        .map(|_| name)
                }
            };
            match checked {
                Ok(name) => {
                    files
                        .entry(name.clone())
                        .or_insert_with(|| PathBuf::from(&path));
                    artwork.insert((path, hash), name);
                }
                Err(message) => {
                    left_out += 1;
                    let _ = parlour_jobs::add_error(
                        pool,
                        job_id,
                        Some(&path),
                        &format!("artwork left out of the bundle: {message}"),
                    )
                    .await;
                }
            }
            if done % 25 == 24 {
                let _ = parlour_jobs::set_progress(pool, job_id, (done + 1) as i32, total).await;
            }
        }
    }

    let bundle = build(&snapshot, options, &artwork)?;
    let named = bundle.artwork_files();
    files.retain(|name, _| named.contains(name.as_str()));

    tokio::fs::create_dir_all(export_dir)
        .await
        .map_err(|e| format!("{}: {e}", export_dir.display()))?;
    let dest = bundle_path(export_dir, job_id);
    let summary = ExportSummary {
        file_name: download_name(bundle.manifest.exported_at),
        bytes: 0,
        hashes_included: bundle.manifest.hashes_included,
        artwork_included: bundle.manifest.artwork_included,
        users: bundle.users.users.len(),
        profiles: bundle.profiles.profiles.len(),
        libraries: bundle.libraries.libraries.len(),
        items: bundle.catalog.items.len(),
        watch_state: bundle
            .user_data
            .profiles
            .iter()
            .map(|p| p.items.len())
            .sum(),
        recent_searches: bundle
            .profiles
            .profiles
            .iter()
            .map(|p| p.recent_searches.len())
            .sum(),
        activity: bundle.user_data.activity.len(),
        artwork_files: files.len(),
        artwork_left_out: left_out,
        duplicate_images_left_out: duplicates.len(),
    };
    let bytes = tokio::task::spawn_blocking(move || write_bundle(&dest, &bundle, &files))
        .await
        .map_err(|e| e.to_string())??;
    Ok(ExportSummary { bytes, ..summary })
}

/// Run one claimed `export_bundle` job: write the bundle, record the
/// summary, and note in the Activity feed who exported what (the
/// feed says whether hashes went into the file).
pub async fn run_export_job_claimed(
    pool: parlour_db::PgPool,
    job: parlour_jobs::Job,
    export_dir: &Path,
) {
    let options = ExportOptions::from_payload(&job.payload);
    let requested_by = job.payload["requested_by"]["user_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());
    let requester = job.payload["requested_by"]["username"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let entry = |kind: &'static str, summary: &str| match requested_by {
        Some(user_id) => crate::activity::Entry::by_user(kind, user_id, &requester, summary),
        None => crate::activity::Entry::system(kind, summary),
    };
    match export(&pool, job.id, options, export_dir).await {
        Ok(summary) => {
            tracing::info!(job_id = %job.id, bytes = summary.bytes,
                hashes = summary.hashes_included, artwork = summary.artwork_included,
                "server export finished");
            let detail = serde_json::json!({
                "job_id": job.id,
                "hashes_included": summary.hashes_included,
                "artwork_included": summary.artwork_included,
                "bytes": summary.bytes,
            });
            let _ = parlour_jobs::set_result(
                &pool,
                job.id,
                serde_json::to_value(&summary).unwrap_or(serde_json::Value::Null),
            )
            .await;
            let _ = parlour_jobs::complete(&pool, job.id).await;
            let text = if summary.hashes_included {
                "Server export finished, with password and PIN hashes"
            } else {
                "Server export finished"
            };
            crate::activity::record(&pool, entry("export.completed", text).detail(detail)).await;
        }
        Err(message) => {
            tracing::warn!(job_id = %job.id, error = %message, "server export failed");
            let _ = parlour_jobs::fail(&pool, job.id, &message).await;
            crate::activity::record(
                &pool,
                entry("export.failed", "Server export failed")
                    .detail(serde_json::json!({ "job_id": job.id })),
            )
            .await;
        }
    }
}

/// Where export bundles are kept: `<cache dir>/exports`.
pub fn export_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("exports")
}

/// The worker for export jobs left queued (the request that queued
/// one normally runs it at once). Called once from `main`.
pub fn spawn_worker(pool: parlour_db::PgPool, export_dir: PathBuf) {
    tokio::spawn(async move {
        loop {
            match parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::ExportBundle).await {
                Ok(Some(job)) => run_export_job_claimed(pool.clone(), job, &export_dir).await,
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(3)).await,
                Err(e) => {
                    tracing::warn!(error = %e, "export job claim failed");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    });
}

// ------------------------------------------------------------ endpoints

/// What opting into artwork would add to a bundle right now.
#[derive(Debug, Serialize)]
struct Estimate {
    /// Distinct cached files that would go in.
    artwork_files: u64,
    /// Their total size in bytes (what the warning shows).
    artwork_bytes: u64,
    /// Cached images whose file is missing or can't be named; they
    /// would travel as provider references only.
    artwork_unavailable: u64,
}

async fn estimate(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Estimate>, AppError> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT path, hash FROM images WHERE source_url IS NOT NULL")
            .fetch_all(&app.pool)
            .await
            .map_err(AppError::from)?;
    let estimate = tokio::task::spawn_blocking(move || {
        let mut named: BTreeMap<String, String> = BTreeMap::new();
        let mut unavailable = 0u64;
        for (path, hash) in rows {
            let image = ImageRow {
                item_id: Uuid::nil(),
                kind: String::new(),
                variant: String::new(),
                source_url: String::new(),
                path,
                hash,
            };
            let Some((path, hash)) = cached_key(&image) else {
                continue;
            };
            match artwork_name(&path, &hash) {
                Some(name) => {
                    named.entry(name).or_insert(path);
                }
                None => unavailable += 1,
            }
        }
        let mut files = 0u64;
        let mut bytes = 0u64;
        for path in named.values() {
            match std::fs::metadata(path) {
                Ok(meta) if meta.is_file() => {
                    files += 1;
                    bytes += meta.len();
                }
                _ => unavailable += 1,
            }
        }
        Estimate {
            artwork_files: files,
            artwork_bytes: bytes,
            artwork_unavailable: unavailable,
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(estimate))
}

async fn start_export(
    State(app): State<AppState>,
    admin: AdminUser,
    body: Option<Json<ExportOptions>>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let options = body.map(|Json(options)| options).unwrap_or_default();
    let job_id = parlour_jobs::enqueue(
        &app.pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        serde_json::json!({
            "trigger": "manual",
            "include_hashes": options.include_hashes,
            "include_artwork": options.include_artwork,
            "requested_by": { "user_id": admin.0.user_id, "username": admin.0.username },
        }),
    )
    .await
    .map_err(AppError::from)?;
    // Run it now; the background worker is the backstop if the
    // process dies before this task claims it.
    let pool = app.pool.clone();
    let export_dir = app.export_dir.clone();
    tokio::spawn(async move {
        if parlour_jobs::claim_by_id(&pool, job_id)
            .await
            .unwrap_or(false)
        {
            if let Ok(Some(job)) = parlour_jobs::get(&pool, job_id).await {
                run_export_job_claimed(pool, job, &export_dir).await;
            }
        }
    });
    let job = parlour_jobs::get(&app.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished after enqueue".into()))?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// The export job behind `{id}`, finished: the download and the
/// delete both need one.
async fn finished_export(app: &AppState, id: &str) -> Result<parlour_jobs::Job, AppError> {
    let job_id = Uuid::parse_str(id).map_err(|_| AppError::BadRequest("invalid id".into()))?;
    let job = parlour_jobs::get(&app.pool, job_id)
        .await
        .map_err(AppError::from)?
        .filter(|job| job.kind == parlour_jobs::JobKind::ExportBundle.as_str())
        .ok_or_else(|| AppError::NotFound("no such export".into()))?;
    match job.status.as_str() {
        "succeeded" => Ok(job),
        "failed" => Err(AppError::NotFound(
            "that export failed; it has no bundle".into(),
        )),
        _ => Err(AppError::Conflict("that export hasn't finished yet".into())),
    }
}

async fn download(
    State(app): State<AppState>,
    _admin: AdminUser,
    AxumPath(id): AxumPath<String>,
) -> Result<Response, AppError> {
    let job = finished_export(&app, &id).await?;
    let gone = || AppError::NotFound("that bundle has been deleted from the server".into());
    let file = tokio::fs::File::open(bundle_path(&app.export_dir, job.id))
        .await
        .map_err(|_| gone())?;
    let size = file
        .metadata()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .len();
    // The name came from `download_name`; anything else falls back.
    let name = job
        .result
        .as_ref()
        .and_then(|r| r["file_name"].as_str())
        .filter(|n| {
            n.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
        .unwrap_or("parlour-export.zip")
        .to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, size)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
        .map_err(|e| AppError::Internal(e.to_string()))
}

async fn delete_bundle(
    State(app): State<AppState>,
    _admin: AdminUser,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let job = finished_export(&app, &id).await?;
    match tokio::fs::remove_file(bundle_path(&app.export_dir, job.id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::NotFound(
                "that bundle has already been deleted".into(),
            ))
        }
        Err(e) => return Err(AppError::Internal(e.to_string())),
    }
    sqlx::query(
        "UPDATE jobs SET result = COALESCE(result, '{}'::jsonb) || '{\"deleted\": true}'::jsonb
         WHERE id = $1",
    )
    .bind(job.id)
    .execute(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/export", post(start_export))
        .route("/api/v1/admin/export/estimate", get(estimate))
        .route("/api/v1/admin/export/{id}/download", get(download))
        .route("/api/v1/admin/export/{id}", delete(delete_bundle))
}

#[cfg(test)]
mod tests;
