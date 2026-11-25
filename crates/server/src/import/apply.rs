//! One bundle against the database (XFER3; spec
//! `docs/IMPORT-EXPORT.md` §12).
//!
//! [`run`] walks the bundle in the spec's insert order — users;
//! profiles; libraries, paths and access; items, parents first;
//! people, segments and images; watch state, recent searches and
//! Activity — on one connection that the caller holds inside a
//! transaction. The dry run is the very same walk, rolled back, so
//! the report it shows is exactly what the apply job will do (as long
//! as nothing else changes in between).
//!
//! The rules (each is in §12 with its reasons):
//!
//! - **Never delete.** Nothing here issues a `DELETE`. An existing
//!   row changes only when the bundle's copy is newer by the row's
//!   own clock (watch state and segments by `updated_at`, recent
//!   searches by `searched_at`, an item's metadata by
//!   `last_metadata_at`). Rows with no clock (accounts, profiles,
//!   grants, people) are only ever added.
//! - **Users match by username, ignoring case.** The same account
//!   (same username *and* the same `created_at`, to the microsecond)
//!   is matched; any other holder of the name is a conflict, reported
//!   and never merged — except D38's pristine bootstrap admin, which
//!   the bundle's admin adopts.
//! - **D37:** a user arriving without a hash is created locked
//!   ([`crate::auth::locked_hash`]); an admin sets their password.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{is_locked_hash, locked_hash};
use crate::bundle::{
    parse_artwork_name, Bundle, ItemRef, LibraryKind, Manifest, MetadataStatus, ProfileRef,
};

// ------------------------------------------------------------ options

/// What the admin may change about an import. Everything else comes
/// from the bundle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    /// Root folders to give a library instead of the bundle's, which
    /// are the old host's. By library name. For a library this server
    /// already has, the folders are added to its own (none is ever
    /// removed); without an entry here an existing library keeps just
    /// its own folders.
    #[serde(default)]
    pub library_paths: BTreeMap<String, Vec<String>>,
}

impl ImportOptions {
    /// Every name must be a library in the bundle, and every path
    /// absolute (the rule `POST /api/v1/admin/libraries` applies).
    pub fn check(&self, bundle: &Bundle) -> Result<(), String> {
        for (name, paths) in &self.library_paths {
            if !bundle.libraries.libraries.iter().any(|l| &l.name == name) {
                return Err(format!("the bundle has no library named '{name}'"));
            }
            for path in paths {
                if path.trim().is_empty() || !path.trim().starts_with('/') {
                    return Err(format!("library paths must be absolute: {path:?}"));
                }
            }
        }
        Ok(())
    }
}

/// The dry run, or the real thing with the bundle's artwork already
/// extracted (and hash-checked) to `staged`: artwork name → file.
pub enum Mode<'a> {
    DryRun,
    Apply {
        staged: &'a BTreeMap<String, PathBuf>,
        artwork_dir: &'a Path,
    },
}

impl Mode<'_> {
    fn is_dry_run(&self) -> bool {
        matches!(self, Mode::DryRun)
    }
}

/// Where [`run`] reports how far it has got (the job's progress bar).
pub struct Progress<'a> {
    pub pool: &'a parlour_db::PgPool,
    pub job_id: Uuid,
}

/// The steps [`run`] reports progress over.
const STEPS: i32 = 8;

// ------------------------------------------------------------ the report

/// How many rows of one kind were (or, in a dry run, would be)
/// created, changed, found already here, or skipped because what they
/// belong to was not imported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub created: u64,
    pub updated: u64,
    /// Already here: identical, or the server's copy is kept (it is
    /// as new or newer, or the row has no clock to compare).
    pub existing: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAction {
    /// A new account.
    Create,
    /// This server already has the same account (same username and
    /// creation time): an earlier import of this bundle, or the
    /// server it came from.
    Existing,
    /// D38: the bundle's admin takes over this server's untouched
    /// bootstrap admin, which has the same name.
    Adopt,
    /// Another account here holds the name. Reported, not merged: the
    /// user and everything of theirs is skipped.
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignIn {
    /// The bundle's hash: the old password works.
    FromBundle,
    /// D37: no hash in the bundle, so the account is created locked
    /// until an admin sets a password (Server → Users).
    Locked,
    /// The account's password here is left as it is.
    Kept,
    /// Not imported (a conflict).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserOutcome {
    pub username: String,
    pub is_admin: bool,
    pub action: UserAction,
    /// How the account signs in after the import.
    pub sign_in: SignIn,
    /// The account here it matched, adopted or clashed with, when its
    /// name differs in case.
    pub existing_username: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryAction {
    Create,
    Existing,
    /// A library here has the name but another kind.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathCheck {
    pub path: String,
    /// Whether the folder exists on this host (as the server sees it).
    pub exists_here: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryOutcome {
    pub name: String,
    pub kind: LibraryKind,
    pub action: LibraryAction,
    /// The bundle's root folders: the old host's.
    pub bundle_paths: Vec<PathCheck>,
    /// The folders this import gives the library (the bundle's, or
    /// the admin's replacements). For an existing library, only those
    /// it doesn't have yet.
    pub paths_added: Vec<PathCheck>,
    pub note: Option<String>,
}

/// Something the import won't do, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// `user`, `library` or `item`.
    pub what: String,
    /// The username, library name, or `library / stable_key`.
    pub key: String,
    pub reason: String,
}

/// At most this many conflicts are listed; `conflicts_total` counts
/// them all.
const MAX_LISTED_CONFLICTS: usize = 500;

/// What an import does (or, from a dry run, would do).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub dry_run: bool,
    pub manifest: Manifest,
    pub users: Vec<UserOutcome>,
    pub libraries: Vec<LibraryOutcome>,
    pub conflicts: Vec<Conflict>,
    pub conflicts_total: u64,
    pub profiles: Tally,
    pub access: Tally,
    pub items: Tally,
    /// Of `items.updated` + `items.existing`: how many matched on a
    /// provider id because their `stable_key` didn't.
    pub items_matched_by_provider_id: u64,
    pub people: Tally,
    pub credits: Tally,
    pub segments: Tally,
    pub images: Tally,
    /// Artwork files written into this server's artwork cache.
    pub artwork_files: Tally,
    pub watch_state: Tally,
    pub recent_searches: Tally,
    pub activity: Tally,
    /// Plain sentences the admin should read before applying.
    pub notes: Vec<String>,
}

impl Report {
    fn new(manifest: &Manifest, dry_run: bool) -> Self {
        Self {
            dry_run,
            manifest: manifest.clone(),
            users: Vec::new(),
            libraries: Vec::new(),
            conflicts: Vec::new(),
            conflicts_total: 0,
            profiles: Tally::default(),
            access: Tally::default(),
            items: Tally::default(),
            items_matched_by_provider_id: 0,
            people: Tally::default(),
            credits: Tally::default(),
            segments: Tally::default(),
            images: Tally::default(),
            artwork_files: Tally::default(),
            watch_state: Tally::default(),
            recent_searches: Tally::default(),
            activity: Tally::default(),
            notes: Vec::new(),
        }
    }

    fn conflict(&mut self, what: &str, key: String, reason: String) {
        self.conflicts_total += 1;
        if self.conflicts.len() < MAX_LISTED_CONFLICTS {
            self.conflicts.push(Conflict {
                what: what.to_string(),
                key,
                reason,
            });
        }
    }
}

/// What an upsert's `RETURNING (xmax = 0)` says: a row back means it
/// was inserted (`true`) or updated (`false`); no row means the
/// `WHERE` on `DO UPDATE` kept the existing one.
fn count(tally: &mut Tally, returned: Option<bool>) {
    match returned {
        Some(true) => tally.created += 1,
        Some(false) => tally.updated += 1,
        None => tally.existing += 1,
    }
}

// ------------------------------------------------------------ pure rules

/// Whether the bundle's item metadata replaces the server's: only
/// when the bundle's was matched more recently (`last_metadata_at`).
/// Both unmatched: the server's scan is kept.
pub(crate) fn bundle_metadata_is_newer(
    bundle: Option<DateTime<Utc>>,
    here: Option<DateTime<Utc>>,
) -> bool {
    match (bundle, here) {
        (Some(b), Some(h)) => b > h,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// An image variant safe to put in a file name; others travel as a
/// provider reference only.
pub(crate) fn variant_is_file_safe(variant: &str) -> bool {
    (1..=32).contains(&variant.len())
        && variant
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The path an imported artwork file gets in the artwork cache:
/// `<artwork dir>/<item id>/<kind>-<variant>-<first 16 of sha256>.<ext>`.
/// The hash in the name keeps it clear of the file the download job
/// writes for the same slot.
pub(crate) fn artwork_dest(
    artwork_dir: &Path,
    item_id: Uuid,
    kind: &str,
    variant: &str,
    file: &str,
) -> Option<PathBuf> {
    let (sha256, ext) = parse_artwork_name(file)?;
    variant_is_file_safe(variant).then(|| {
        artwork_dir
            .join(item_id.to_string())
            .join(format!("{kind}-{variant}-{}.{ext}", &sha256[..16]))
    })
}

/// Items ordered so every parent comes before its children (the
/// bundle guarantees parents exist and never loop).
pub(crate) fn parents_first(bundle: &Bundle) -> Vec<usize> {
    let index: HashMap<(&str, &str), usize> = bundle
        .catalog
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| ((item.library.as_str(), item.stable_key.as_str()), i))
        .collect();
    let depth = |mut i: usize| {
        let mut d = 0usize;
        while let Some(parent) = bundle.catalog.items[i].parent.as_deref() {
            match index.get(&(bundle.catalog.items[i].library.as_str(), parent)) {
                Some(&p) if d < bundle.catalog.items.len() => {
                    i = p;
                    d += 1;
                }
                _ => break,
            }
        }
        d
    };
    let mut order: Vec<usize> = (0..bundle.catalog.items.len()).collect();
    order.sort_by_key(|&i| depth(i));
    order
}

fn is_real_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with("pending:")
}

fn path_check(path: &str) -> PathCheck {
    PathCheck {
        path: path.to_string(),
        exists_here: Path::new(path).is_dir(),
    }
}

// ------------------------------------------------------------ the walk

#[derive(Debug, Clone, sqlx::FromRow)]
struct HereUser {
    id: Uuid,
    username: String,
    password_hash: String,
    is_admin: bool,
    created_at: DateTime<Utc>,
    disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct HereItem {
    id: Uuid,
    library_id: Uuid,
    stable_key: String,
    kind: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    imdb_id: Option<String>,
    last_metadata_at: Option<DateTime<Utc>>,
}

/// Ids on this server for the bundle's names, filled in step by step.
#[derive(Default)]
struct Placed {
    users: HashMap<String, Uuid>,
    profiles: HashMap<ProfileRef, Uuid>,
    libraries: HashMap<String, Uuid>,
    /// Items, and whether the bundle's metadata went in (so its
    /// credits and images follow it).
    items: HashMap<ItemRef, (Uuid, bool)>,
}

/// Import `bundle` on `conn`, which the caller holds inside a
/// transaction it commits (apply) or rolls back (dry run). Returns the
/// report; an error means the caller must roll back.
pub async fn run(
    conn: &mut PgConnection,
    bundle: &Bundle,
    options: &ImportOptions,
    mode: Mode<'_>,
    progress: Option<Progress<'_>>,
) -> Result<Report, String> {
    options.check(bundle)?;
    let mut report = Report::new(&bundle.manifest, mode.is_dry_run());
    let mut placed = Placed::default();
    let step = |n: i32| {
        let progress = progress.as_ref().map(|p| (p.pool.clone(), p.job_id));
        async move {
            if let Some((pool, job_id)) = progress {
                let _ = parlour_jobs::set_progress(&pool, job_id, n, STEPS).await;
            }
        }
    };
    // One import at a time: two at once would race on the same names.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('parlour.import_bundle'))")
        .execute(&mut *conn)
        .await
        .map_err(|e| format!("waiting for another import: {e}"))?;

    step(0).await;
    let adopted_profile = users(conn, bundle, &mut report, &mut placed).await?;
    step(1).await;
    profiles(conn, bundle, &mut report, &mut placed, adopted_profile).await?;
    step(2).await;
    libraries(conn, bundle, options, &mut report, &mut placed).await?;
    step(3).await;
    items(conn, bundle, &mut report, &mut placed).await?;
    step(4).await;
    catalog_rows(conn, bundle, &mode, &mut report, &placed).await?;
    step(5).await;
    watch_state(conn, bundle, &mut report, &placed).await?;
    step(6).await;
    recent_searches(conn, bundle, &mut report, &placed).await?;
    step(7).await;
    activity(conn, bundle, &mut report, &placed).await?;
    step(STEPS).await;
    notes(bundle, &mut report);
    Ok(report)
}

/// D38: whether `here` is a pristine bootstrap admin the bundle's
/// admin may adopt. The bootstrap only ever creates the first user,
/// so it must be the server's first account (`first`, taken before
/// this import added any); an admin, not disabled;
/// still holding the one profile `create_user` gave it, named as the
/// account was at creation (a rename leaves the profile's name
/// behind) and with no PIN; and with no watch state, audiobook
/// progress or recent searches of its own. Returns that profile's id.
async fn pristine_bootstrap(
    conn: &mut PgConnection,
    here: &HereUser,
    first: Option<Uuid>,
) -> Result<Option<Uuid>, sqlx::Error> {
    if first != Some(here.id) || !here.is_admin || here.disabled_at.is_some() {
        return Ok(None);
    }
    let profiles: Vec<(Uuid, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, pin_hash FROM profiles WHERE user_id = $1")
            .bind(here.id)
            .fetch_all(&mut *conn)
            .await?;
    let [(profile_id, name, pin)] = profiles.as_slice() else {
        return Ok(None);
    };
    if name != &here.username || pin.is_some() {
        return Ok(None);
    }
    let (state,): (i64,) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM user_data WHERE profile_id = $1)
              + (SELECT count(*) FROM book_progress WHERE profile_id = $1)
              + (SELECT count(*) FROM recent_searches WHERE profile_id = $1)",
    )
    .bind(profile_id)
    .fetch_one(&mut *conn)
    .await?;
    Ok((state == 0).then_some(*profile_id))
}

/// Step 1: users. Returns the profile of an adopted bootstrap admin,
/// which takes the bundle's same-named profile's settings.
async fn users(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &mut Placed,
) -> Result<Option<(Uuid, String)>, String> {
    let here: Vec<HereUser> = sqlx::query_as(
        "SELECT id, username, password_hash, is_admin, created_at, disabled_at FROM users",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| format!("reading users: {e}"))?;
    // The server's first account, before this import adds any.
    let first = here
        .iter()
        .min_by_key(|u| (u.created_at, u.id))
        .map(|u| u.id);
    let by_name: HashMap<String, &HereUser> = here
        .iter()
        .map(|u| (u.username.to_lowercase(), u))
        .collect();
    let mut adopted_profile = None;
    for user in &bundle.users.users {
        let other_name = |h: &HereUser| (h.username != user.username).then(|| h.username.clone());
        let mut outcome = UserOutcome {
            username: user.username.clone(),
            is_admin: user.is_admin,
            action: UserAction::Create,
            sign_in: SignIn::Kept,
            existing_username: None,
            note: None,
        };
        match by_name.get(&user.username.to_lowercase()) {
            None => {
                let (hash, password) = match &user.password_hash {
                    Some(hash) => (hash.clone(), SignIn::FromBundle),
                    None => (locked_hash(), SignIn::Locked),
                };
                let (id,): (Uuid,) = sqlx::query_as(
                    "INSERT INTO users (username, password_hash, is_admin, created_at, disabled_at)
                     VALUES ($1, $2, $3, $4, $5) RETURNING id",
                )
                .bind(&user.username)
                .bind(&hash)
                .bind(user.is_admin)
                .bind(user.created_at)
                .bind(user.disabled_at)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| format!("creating user '{}': {e}", user.username))?;
                outcome.sign_in = password;
                placed.users.insert(user.username.clone(), id);
            }
            Some(h) if h.created_at == user.created_at => {
                outcome.action = UserAction::Existing;
                outcome.existing_username = other_name(h);
                // A lock (an earlier import without hashes) is filled
                // in; a real password here is never replaced.
                if let (true, Some(hash)) = (is_locked_hash(&h.password_hash), &user.password_hash)
                {
                    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
                        .bind(h.id)
                        .bind(hash)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| format!("updating user '{}': {e}", user.username))?;
                    outcome.sign_in = SignIn::FromBundle;
                    outcome.note = Some("its password, locked here, comes from the bundle".into());
                } else if is_locked_hash(&h.password_hash) {
                    outcome.sign_in = SignIn::Locked;
                }
                placed.users.insert(user.username.clone(), h.id);
            }
            Some(h) => {
                let pristine = if user.is_admin && user.disabled_at.is_none() {
                    pristine_bootstrap(conn, h, first)
                        .await
                        .map_err(|e| format!("checking user '{}': {e}", h.username))?
                } else {
                    None
                };
                outcome.existing_username = other_name(h);
                if let Some(profile_id) = pristine {
                    sqlx::query(
                        "UPDATE users SET username = $2, created_at = $3, is_admin = true,
                                          password_hash = COALESCE($4, password_hash)
                         WHERE id = $1",
                    )
                    .bind(h.id)
                    .bind(&user.username)
                    .bind(user.created_at)
                    .bind(&user.password_hash)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| format!("adopting user '{}': {e}", user.username))?;
                    outcome.action = UserAction::Adopt;
                    outcome.sign_in = if user.password_hash.is_some() {
                        SignIn::FromBundle
                    } else {
                        SignIn::Kept
                    };
                    outcome.note = Some(format!(
                        "this server's untouched first admin, '{}', becomes the bundle's admin (D38); {}",
                        h.username,
                        if user.password_hash.is_some() {
                            "the bundle's password replaces the bootstrap one"
                        } else {
                            "it keeps the bootstrap password"
                        }
                    ));
                    placed.users.insert(user.username.clone(), h.id);
                    adopted_profile = Some((profile_id, h.username.clone()));
                } else {
                    outcome.action = UserAction::Conflict;
                    outcome.sign_in = SignIn::None;
                    let reason = format!(
                        "another account here, '{}' (created {}), already has this username",
                        h.username,
                        h.created_at.format("%Y-%m-%d")
                    );
                    report.conflict("user", user.username.clone(), reason.clone());
                    outcome.note = Some(reason);
                }
            }
        }
        report.users.push(outcome);
    }
    Ok(adopted_profile)
}

/// The PIN hash a profile is written with: the bundle's when it has
/// one; D37's lock when the profile has a PIN but the bundle carries
/// no hash (so it isn't left open); otherwise none.
fn pin_for(bundle: &Bundle, username: &str, profile: &str, has_pin: bool) -> Option<String> {
    let carried = bundle
        .users
        .users
        .iter()
        .find(|u| u.username == username)
        .and_then(|u| u.pin_hashes.as_ref())
        .and_then(|pins| pins.get(profile))
        .cloned();
    match carried {
        Some(hash) => Some(hash),
        None if has_pin => Some(locked_hash()),
        None => None,
    }
}

/// Step 2: profiles of the users placed in step 1.
async fn profiles(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &mut Placed,
    adopted_profile: Option<(Uuid, String)>,
) -> Result<(), String> {
    let user_ids: Vec<Uuid> = placed.users.values().copied().collect();
    let here: Vec<(Uuid, Uuid, String, Option<String>)> =
        sqlx::query_as("SELECT id, user_id, name, pin_hash FROM profiles WHERE user_id = ANY($1)")
            .bind(&user_ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| format!("reading profiles: {e}"))?;
    let by_key: HashMap<(Uuid, &str), (Uuid, Option<&str>)> = here
        .iter()
        .map(|(id, user, name, pin)| ((*user, name.as_str()), (*id, pin.as_deref())))
        .collect();
    for profile in &bundle.profiles.profiles {
        let Some(&user_id) = placed.users.get(&profile.username) else {
            report.profiles.skipped += 1;
            continue;
        };
        let key = ProfileRef {
            username: profile.username.clone(),
            profile: profile.name.clone(),
        };
        let prefs = &profile.preferences;
        let pin = pin_for(bundle, &profile.username, &profile.name, profile.has_pin);
        match by_key.get(&(user_id, profile.name.as_str())) {
            Some(&(id, _)) if adopted_profile.as_ref().is_some_and(|(p, _)| *p == id) => {
                // D38: the adopted admin's untouched default profile
                // becomes the bundle's.
                sqlx::query(
                    "UPDATE profiles SET pin_hash = $2, avatar_color = $3, audio_lang = $4,
                            subtitle_lang = $5, subtitle_mode = $6, remote_bitrate_cap_kbps = $7,
                            created_at = $8, home_rail_order = $9, home_rails_hidden = $10,
                            segment_skip = $11, autoplay_next = $12
                     WHERE id = $1",
                )
                .bind(id)
                .bind(&pin)
                .bind(&profile.avatar_color)
                .bind(&prefs.audio_lang)
                .bind(&prefs.subtitle_lang)
                .bind(prefs.subtitle_mode.as_str())
                .bind(prefs.remote_bitrate_cap_kbps)
                .bind(profile.created_at)
                .bind(&prefs.home_rail_order)
                .bind(&prefs.home_rails_hidden)
                .bind(prefs.segment_skip.as_str())
                .bind(prefs.autoplay_next)
                .execute(&mut *conn)
                .await
                .map_err(|e| format!("updating profile '{}': {e}", profile.name))?;
                report.profiles.updated += 1;
                placed.profiles.insert(key, id);
            }
            Some(&(id, here_pin)) => {
                // Kept as it is — but a locked PIN is filled in from
                // the bundle when it carries the hash.
                let fill = here_pin.is_some_and(is_locked_hash)
                    && pin.as_deref().is_some_and(|p| !is_locked_hash(p));
                if fill {
                    sqlx::query("UPDATE profiles SET pin_hash = $2 WHERE id = $1")
                        .bind(id)
                        .bind(&pin)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| format!("updating profile '{}': {e}", profile.name))?;
                    report.profiles.updated += 1;
                } else {
                    report.profiles.existing += 1;
                }
                placed.profiles.insert(key, id);
            }
            None => {
                let (id,): (Uuid,) = sqlx::query_as(
                    "INSERT INTO profiles (user_id, name, pin_hash, avatar_color, audio_lang,
                                           subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps,
                                           created_at, home_rail_order, home_rails_hidden,
                                           segment_skip, autoplay_next)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                     RETURNING id",
                )
                .bind(user_id)
                .bind(&profile.name)
                .bind(&pin)
                .bind(&profile.avatar_color)
                .bind(&prefs.audio_lang)
                .bind(&prefs.subtitle_lang)
                .bind(prefs.subtitle_mode.as_str())
                .bind(prefs.remote_bitrate_cap_kbps)
                .bind(profile.created_at)
                .bind(&prefs.home_rail_order)
                .bind(&prefs.home_rails_hidden)
                .bind(prefs.segment_skip.as_str())
                .bind(prefs.autoplay_next)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| format!("creating profile '{}': {e}", profile.name))?;
                report.profiles.created += 1;
                placed.profiles.insert(key, id);
            }
        }
    }
    Ok(())
}

/// Step 3: libraries, their root folders and access grants.
async fn libraries(
    conn: &mut PgConnection,
    bundle: &Bundle,
    options: &ImportOptions,
    report: &mut Report,
    placed: &mut Placed,
) -> Result<(), String> {
    let here: Vec<(Uuid, String, String)> = sqlx::query_as("SELECT id, name, kind FROM libraries")
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| format!("reading libraries: {e}"))?;
    let by_name: HashMap<&str, (Uuid, &str)> = here
        .iter()
        .map(|(id, name, kind)| (name.as_str(), (*id, kind.as_str())))
        .collect();
    for library in &bundle.libraries.libraries {
        let mut outcome = LibraryOutcome {
            name: library.name.clone(),
            kind: library.kind,
            action: LibraryAction::Create,
            bundle_paths: library.paths.iter().map(|p| path_check(p)).collect(),
            paths_added: Vec::new(),
            note: None,
        };
        let replacement = options.library_paths.get(&library.name);
        let (id, wanted): (Uuid, Vec<String>) = match by_name.get(library.name.as_str()) {
            Some(&(id, kind)) if kind == library.kind.as_str() => {
                outcome.action = LibraryAction::Existing;
                // An existing library has its own folders; the old
                // host's are added only when the admin names them.
                (id, replacement.cloned().unwrap_or_default())
            }
            Some(&(_, kind)) => {
                outcome.action = LibraryAction::Conflict;
                let reason = format!(
                    "a library here has this name but holds {kind}, not {}",
                    library.kind.as_str()
                );
                report.conflict("library", library.name.clone(), reason.clone());
                outcome.note = Some(format!(
                    "{reason}; its items, grants and their watch state are skipped"
                ));
                report.access.skipped += library.access.len() as u64;
                report.libraries.push(outcome);
                continue;
            }
            None => {
                let (id,): (Uuid,) = sqlx::query_as(
                    "INSERT INTO libraries (name, kind, created_at) VALUES ($1, $2, $3)
                     RETURNING id",
                )
                .bind(&library.name)
                .bind(library.kind.as_str())
                .bind(library.created_at)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| format!("creating library '{}': {e}", library.name))?;
                (id, replacement.unwrap_or(&library.paths).clone())
            }
        };
        for path in wanted.iter().map(|p| p.trim()) {
            let added = sqlx::query(
                "INSERT INTO library_paths (library_id, path) VALUES ($1, $2)
                 ON CONFLICT (library_id, path) DO NOTHING",
            )
            .bind(id)
            .bind(path)
            .execute(&mut *conn)
            .await
            .map_err(|e| format!("adding a folder to '{}': {e}", library.name))?;
            if added.rows_affected() == 1 {
                outcome.paths_added.push(path_check(path));
            }
        }
        if outcome.paths_added.iter().any(|p| !p.exists_here) {
            outcome.note = Some(
                "a folder it is given doesn't exist on this host; its items stay unavailable until a scan finds their files"
                    .into(),
            );
        }
        placed.libraries.insert(library.name.clone(), id);
        for grant in &library.access {
            let profile = ProfileRef {
                username: grant.username.clone(),
                profile: grant.profile.clone(),
            };
            let Some(&profile_id) = placed.profiles.get(&profile) else {
                report.access.skipped += 1;
                continue;
            };
            let added = sqlx::query(
                "INSERT INTO library_access (profile_id, library_id, max_rating)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (profile_id, library_id) DO NOTHING",
            )
            .bind(profile_id)
            .bind(id)
            .bind(&grant.max_rating)
            .execute(&mut *conn)
            .await
            .map_err(|e| format!("granting '{}': {e}", library.name))?;
            if added.rows_affected() == 1 {
                report.access.created += 1;
            } else {
                report.access.existing += 1;
            }
        }
        report.libraries.push(outcome);
    }
    Ok(())
}

/// Step 4: catalog items, parents first. An item matches on
/// (library, `stable_key`), else on a provider id within its library
/// and kind; a new item is created unavailable (its files are the
/// next scan's to find).
async fn items(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &mut Placed,
) -> Result<(), String> {
    let library_ids: Vec<Uuid> = placed.libraries.values().copied().collect();
    let here: Vec<HereItem> = sqlx::query_as(
        "SELECT id, library_id, stable_key, kind, tmdb_id, tvdb_id, imdb_id, last_metadata_at
         FROM media_items WHERE library_id = ANY($1)",
    )
    .bind(&library_ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| format!("reading items: {e}"))?;
    let by_key: HashMap<(Uuid, &str), usize> = here
        .iter()
        .enumerate()
        .map(|(i, item)| ((item.library_id, item.stable_key.as_str()), i))
        .collect();
    // Keys the bundle itself uses, per library: an item here with one
    // of those is that item's, never another's fallback.
    let bundle_keys: HashSet<(Uuid, &str)> = bundle
        .catalog
        .items
        .iter()
        .filter_map(|item| {
            placed
                .libraries
                .get(&item.library)
                .map(|id| (*id, item.stable_key.as_str()))
        })
        .collect();
    let mut claimed: HashSet<usize> = HashSet::new();

    for i in parents_first(bundle) {
        let item = &bundle.catalog.items[i];
        let item_ref = ItemRef {
            library: item.library.clone(),
            stable_key: item.stable_key.clone(),
        };
        let Some(&library_id) = placed.libraries.get(&item.library) else {
            report.items.skipped += 1;
            continue;
        };
        let parent_id = match &item.parent {
            None => None,
            Some(parent) => {
                let parent_ref = ItemRef {
                    library: item.library.clone(),
                    stable_key: parent.clone(),
                };
                match placed.items.get(&parent_ref) {
                    Some(&(id, _)) => Some(id),
                    None => {
                        report.items.skipped += 1;
                        continue;
                    }
                }
            }
        };
        let key = format!("{} / {}", item.library, item.stable_key);
        let mut by_provider = false;
        let matched = match by_key.get(&(library_id, item.stable_key.as_str())) {
            Some(&h) => Some(h),
            None => {
                let candidates: Vec<usize> = here
                    .iter()
                    .enumerate()
                    .filter(|(h, here_item)| {
                        here_item.library_id == library_id
                            && here_item.kind == item.kind.as_str()
                            && !claimed.contains(h)
                            && !bundle_keys.contains(&(library_id, here_item.stable_key.as_str()))
                            && ((item.tmdb_id.is_some() && here_item.tmdb_id == item.tmdb_id)
                                || (item.tvdb_id.is_some() && here_item.tvdb_id == item.tvdb_id)
                                || (item.imdb_id.is_some() && here_item.imdb_id == item.imdb_id))
                    })
                    .map(|(h, _)| h)
                    .collect();
                match candidates.as_slice() {
                    [] => None,
                    [h] => {
                        by_provider = true;
                        Some(*h)
                    }
                    many => {
                        report.conflict(
                            "item",
                            key,
                            format!(
                                "its stable_key isn't here and {} items here share its provider ids",
                                many.len()
                            ),
                        );
                        report.items.skipped += 1;
                        continue;
                    }
                }
            }
        };
        match matched {
            Some(h) if here[h].kind != item.kind.as_str() => {
                report.conflict(
                    "item",
                    key,
                    format!(
                        "the item here with this key is a {}, not a {}",
                        here[h].kind,
                        item.kind.as_str()
                    ),
                );
                report.items.skipped += 1;
            }
            Some(h) => {
                claimed.insert(h);
                if by_provider {
                    report.items_matched_by_provider_id += 1;
                }
                let newer =
                    bundle_metadata_is_newer(item.last_metadata_at, here[h].last_metadata_at);
                if newer {
                    sqlx::query(
                        "UPDATE media_items SET title = $2, sort_title = $3, year = $4,
                                season_number = $5, episode_number = $6, overview = $7,
                                content_rating = $8, tmdb_id = $9, tvdb_id = $10, imdb_id = $11,
                                created_at = LEAST(created_at, $12), updated_at = $13,
                                genres = $14, vote_average = $15, runtime_minutes = $16,
                                air_date = $17, metadata_status = $18, metadata_provider = $19,
                                last_metadata_at = $20
                         WHERE id = $1",
                    )
                    .bind(here[h].id)
                    .bind(&item.title)
                    .bind(&item.sort_title)
                    .bind(item.year)
                    .bind(item.season_number)
                    .bind(item.episode_number)
                    .bind(&item.overview)
                    .bind(&item.content_rating)
                    .bind(item.tmdb_id)
                    .bind(item.tvdb_id)
                    .bind(&item.imdb_id)
                    .bind(item.created_at)
                    .bind(item.updated_at)
                    .bind(&item.genres)
                    .bind(item.vote_average)
                    .bind(item.runtime_minutes)
                    .bind(item.air_date)
                    .bind(item.metadata_status.as_str())
                    .bind(&item.metadata_provider)
                    .bind(item.last_metadata_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| format!("updating {key}: {e}"))?;
                    report.items.updated += 1;
                } else {
                    report.items.existing += 1;
                }
                placed.items.insert(item_ref, (here[h].id, newer));
            }
            None => {
                let (id,): (Uuid,) = sqlx::query_as(
                    "INSERT INTO media_items (library_id, parent_id, kind, stable_key, title,
                                              sort_title, year, season_number, episode_number,
                                              overview, content_rating, tmdb_id, tvdb_id,
                                              imdb_id, created_at, updated_at, genres,
                                              vote_average, runtime_minutes, air_date,
                                              metadata_status, metadata_provider,
                                              last_metadata_at, available)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                             $16, $17, $18, $19, $20, $21, $22, $23, false)
                     RETURNING id",
                )
                .bind(library_id)
                .bind(parent_id)
                .bind(item.kind.as_str())
                .bind(&item.stable_key)
                .bind(&item.title)
                .bind(&item.sort_title)
                .bind(item.year)
                .bind(item.season_number)
                .bind(item.episode_number)
                .bind(&item.overview)
                .bind(&item.content_rating)
                .bind(item.tmdb_id)
                .bind(item.tvdb_id)
                .bind(&item.imdb_id)
                .bind(item.created_at)
                .bind(item.updated_at)
                .bind(&item.genres)
                .bind(item.vote_average)
                .bind(item.runtime_minutes)
                .bind(item.air_date)
                .bind(item.metadata_status.as_str())
                .bind(&item.metadata_provider)
                .bind(item.last_metadata_at)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| format!("creating {key}: {e}"))?;
                report.items.created += 1;
                placed.items.insert(item_ref, (id, true));
            }
        }
    }
    Ok(())
}

/// Step 5: people and credits, segments, and provider artwork. Credits
/// and images go with the bundle's metadata (created or newer items);
/// segments belong to the file, so every placed item takes them,
/// newer wins.
async fn catalog_rows(
    conn: &mut PgConnection,
    bundle: &Bundle,
    mode: &Mode<'_>,
    report: &mut Report,
    placed: &Placed,
) -> Result<(), String> {
    let mut people: HashMap<String, Uuid> = HashMap::new();
    for item in &bundle.catalog.items {
        let item_ref = ItemRef {
            library: item.library.clone(),
            stable_key: item.stable_key.clone(),
        };
        let Some(&(item_id, with_metadata)) = placed.items.get(&item_ref) else {
            report.credits.skipped += item.people.len() as u64;
            report.segments.skipped += item.segments.len() as u64;
            report.images.skipped += item.images.len() as u64;
            continue;
        };
        let key = format!("{} / {}", item.library, item.stable_key);

        for segment in &item.segments {
            let returned: Option<(bool,)> = sqlx::query_as(
                "INSERT INTO media_segments (item_id, kind, source, start_ms, end_ms, confidence,
                                             created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (item_id, kind, source) DO UPDATE
                     SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
                         confidence = EXCLUDED.confidence, created_at = EXCLUDED.created_at,
                         updated_at = EXCLUDED.updated_at
                     WHERE media_segments.updated_at < EXCLUDED.updated_at
                 RETURNING (xmax = 0)",
            )
            .bind(item_id)
            .bind(segment.kind.as_str())
            .bind(segment.source.as_str())
            .bind(segment.start_ms)
            .bind(segment.end_ms)
            .bind(segment.confidence)
            .bind(segment.created_at)
            .bind(segment.updated_at)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| format!("segments of {key}: {e}"))?;
            count(&mut report.segments, returned.map(|r| r.0));
        }

        if !with_metadata {
            report.credits.existing += item.people.len() as u64;
            report.images.existing += item.images.len() as u64;
            continue;
        }

        for credit in &item.people {
            let person_id = match people.get(&credit.name) {
                Some(&id) => id,
                None => {
                    let found: Option<(Uuid, Option<i64>)> =
                        sqlx::query_as("SELECT id, tmdb_id FROM people WHERE name = $1")
                            .bind(&credit.name)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(|e| format!("people of {key}: {e}"))?;
                    let id = match found {
                        Some((id, here_tmdb)) => {
                            if here_tmdb.is_none() && credit.tmdb_id.is_some() {
                                sqlx::query("UPDATE people SET tmdb_id = $2 WHERE id = $1")
                                    .bind(id)
                                    .bind(credit.tmdb_id)
                                    .execute(&mut *conn)
                                    .await
                                    .map_err(|e| format!("people of {key}: {e}"))?;
                                report.people.updated += 1;
                            } else {
                                report.people.existing += 1;
                            }
                            id
                        }
                        None => {
                            let (id,): (Uuid,) = sqlx::query_as(
                                "INSERT INTO people (name, tmdb_id) VALUES ($1, $2) RETURNING id",
                            )
                            .bind(&credit.name)
                            .bind(credit.tmdb_id)
                            .fetch_one(&mut *conn)
                            .await
                            .map_err(|e| format!("people of {key}: {e}"))?;
                            report.people.created += 1;
                            id
                        }
                    };
                    people.insert(credit.name.clone(), id);
                    id
                }
            };
            let returned: Option<(bool,)> = sqlx::query_as(
                "INSERT INTO item_people (item_id, person_id, role, character_name, sort_order)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (item_id, person_id, role) DO UPDATE
                     SET character_name = EXCLUDED.character_name,
                         sort_order = EXCLUDED.sort_order
                     WHERE (item_people.character_name, item_people.sort_order)
                           IS DISTINCT FROM (EXCLUDED.character_name, EXCLUDED.sort_order)
                 RETURNING (xmax = 0)",
            )
            .bind(item_id)
            .bind(person_id)
            .bind(&credit.role)
            .bind(&credit.character_name)
            .bind(credit.sort_order)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| format!("credits of {key}: {e}"))?;
            count(&mut report.credits, returned.map(|r| r.0));
        }

        for image in &item.images {
            let kind = image.kind.as_str();
            let pending = format!("pending:{kind}:{}", image.variant);
            // Where the bundle's file would go, if it has one this
            // import can place.
            let dest = match (&image.file, mode) {
                (Some(file), Mode::Apply { artwork_dir, .. }) => {
                    artwork_dest(artwork_dir, item_id, kind, &image.variant, file)
                }
                (Some(file), Mode::DryRun) => {
                    artwork_dest(Path::new("artwork"), item_id, kind, &image.variant, file)
                }
                (None, _) => None,
            };
            let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
                "SELECT id, source_url, path FROM images
                 WHERE item_id = $1 AND kind = $2 AND variant = $3 AND source_url IS NOT NULL
                 ORDER BY path",
            )
            .bind(item_id)
            .bind(kind)
            .bind(&image.variant)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| format!("images of {key}: {e}"))?;
            let same = rows.iter().find(|(_, url, _)| url == &image.source_url);
            // Already here with its file, or with no file to give it.
            if same.is_some_and(|(_, _, path)| is_real_path(path))
                || (same.is_some() && dest.is_none())
            {
                report.images.existing += 1;
                continue;
            }
            // Place the file now, so the row can point at it.
            let placed_file = match (&dest, &image.file) {
                (Some(dest), Some(file)) => match place_artwork(mode, file, dest).await {
                    Ok(()) => {
                        report.artwork_files.created += 1;
                        Some((
                            dest.display().to_string(),
                            parse_artwork_name(file).map(|(sha, _)| sha.to_string()),
                        ))
                    }
                    Err(_) => {
                        report.artwork_files.skipped += 1;
                        None
                    }
                },
                _ => None,
            };
            let (path, hash) = match placed_file {
                Some((path, hash)) => (path, hash),
                None => (pending.clone(), None),
            };
            let target = same.or_else(|| {
                rows.iter()
                    .find(|(_, _, p)| p == &pending)
                    .or_else(|| rows.first())
            });
            match target {
                Some((id, _, here_path)) => {
                    if same.is_some() && path == *here_path {
                        report.images.existing += 1;
                        continue;
                    }
                    sqlx::query(
                        "UPDATE images SET source_url = $2, path = $3, hash = $4 WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&image.source_url)
                    .bind(&path)
                    .bind(&hash)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| format!("images of {key}: {e}"))?;
                    report.images.updated += 1;
                }
                None => {
                    let added = sqlx::query(
                        "INSERT INTO images (item_id, kind, variant, path, hash, source_url)
                         VALUES ($1, $2, $3, $4, $5, $6)
                         ON CONFLICT (item_id, kind, path) DO NOTHING",
                    )
                    .bind(item_id)
                    .bind(kind)
                    .bind(&image.variant)
                    .bind(&path)
                    .bind(&hash)
                    .bind(&image.source_url)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| format!("images of {key}: {e}"))?;
                    if added.rows_affected() == 1 {
                        report.images.created += 1;
                    } else {
                        report.images.existing += 1;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Copy one staged artwork file into the artwork cache (written next
/// to its place and renamed in, so a reader never sees half a file).
/// A dry run only checks that the bundle holds the file.
async fn place_artwork(mode: &Mode<'_>, file: &str, dest: &Path) -> Result<(), String> {
    let Mode::Apply { staged, .. } = mode else {
        return Ok(());
    };
    let source = staged
        .get(file)
        .ok_or_else(|| format!("{file} was not extracted"))?;
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let part = dest.with_extension("part");
    let copied = tokio::fs::copy(source, &part).await;
    let result = match copied {
        Ok(_) => tokio::fs::rename(&part, dest).await,
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result.map_err(|e| format!("{}: {e}", dest.display()))
}

/// Step 6: watch state. The bundle's row wins only when it is newer.
async fn watch_state(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &Placed,
) -> Result<(), String> {
    for group in &bundle.user_data.profiles {
        let profile = ProfileRef {
            username: group.username.clone(),
            profile: group.profile.clone(),
        };
        let Some(&profile_id) = placed.profiles.get(&profile) else {
            report.watch_state.skipped += group.items.len() as u64;
            continue;
        };
        for row in &group.items {
            let Some(&(item_id, _)) = placed.items.get(&row.item) else {
                report.watch_state.skipped += 1;
                continue;
            };
            let returned: Option<(bool,)> = sqlx::query_as(
                "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played,
                                        is_favorite, play_count, last_played_at, created_at,
                                        updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (profile_id, item_id) DO UPDATE
                     SET position_ms = EXCLUDED.position_ms, duration_ms = EXCLUDED.duration_ms,
                         played = EXCLUDED.played, is_favorite = EXCLUDED.is_favorite,
                         play_count = EXCLUDED.play_count,
                         last_played_at = EXCLUDED.last_played_at,
                         created_at = EXCLUDED.created_at, updated_at = EXCLUDED.updated_at
                     WHERE user_data.updated_at < EXCLUDED.updated_at
                 RETURNING (xmax = 0)",
            )
            .bind(profile_id)
            .bind(item_id)
            .bind(row.position_ms)
            .bind(row.duration_ms)
            .bind(row.played)
            .bind(row.is_favorite)
            .bind(row.play_count)
            .bind(row.last_played_at)
            .bind(row.created_at)
            .bind(row.updated_at)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| format!("watch state of {}/{}: {e}", group.username, group.profile))?;
            count(&mut report.watch_state, returned.map(|r| r.0));
        }
    }
    Ok(())
}

/// Step 7: recent searches (D25), one per query ignoring case; the
/// newer search wins.
async fn recent_searches(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &Placed,
) -> Result<(), String> {
    for profile in &bundle.profiles.profiles {
        let key = ProfileRef {
            username: profile.username.clone(),
            profile: profile.name.clone(),
        };
        let Some(&profile_id) = placed.profiles.get(&key) else {
            report.recent_searches.skipped += profile.recent_searches.len() as u64;
            continue;
        };
        for search in &profile.recent_searches {
            let returned: Option<(bool,)> = sqlx::query_as(
                "INSERT INTO recent_searches (profile_id, query, searched_at) VALUES ($1, $2, $3)
                 ON CONFLICT (profile_id, lower(query)) DO UPDATE
                     SET query = EXCLUDED.query, searched_at = EXCLUDED.searched_at
                     WHERE recent_searches.searched_at < EXCLUDED.searched_at
                 RETURNING (xmax = 0)",
            )
            .bind(profile_id)
            .bind(&search.query)
            .bind(search.searched_at)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| {
                format!(
                    "recent searches of {}/{}: {e}",
                    profile.username, profile.name
                )
            })?;
            count(&mut report.recent_searches, returned.map(|r| r.0));
        }
    }
    Ok(())
}

/// Step 8: the Activity feed (D25). An entry has no key, so one is
/// already here when its own fields — time, kind, actor, summary and
/// detail — all match; its user, profile and item are where it
/// attaches on this server, resolved by name (null when that wasn't
/// imported, as for a deleted account).
async fn activity(
    conn: &mut PgConnection,
    bundle: &Bundle,
    report: &mut Report,
    placed: &Placed,
) -> Result<(), String> {
    let mut detached = 0u64;
    for entry in &bundle.user_data.activity {
        let (present,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM activity
                 WHERE created_at = $1 AND kind = $2 AND actor IS NOT DISTINCT FROM $3
                   AND summary = $4 AND detail = $5
             )",
        )
        .bind(entry.created_at)
        .bind(&entry.kind)
        .bind(&entry.actor)
        .bind(&entry.summary)
        .bind(&entry.detail)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| format!("reading activity: {e}"))?;
        if present {
            report.activity.existing += 1;
            continue;
        }
        let user_id = entry
            .username
            .as_ref()
            .and_then(|u| placed.users.get(u).copied());
        let profile_id = entry
            .profile
            .as_ref()
            .and_then(|p| placed.profiles.get(p).copied());
        let item_id = entry
            .item
            .as_ref()
            .and_then(|i| placed.items.get(i).map(|(id, _)| *id));
        if (entry.username.is_some() && user_id.is_none())
            || (entry.profile.is_some() && profile_id.is_none())
            || (entry.item.is_some() && item_id.is_none())
        {
            detached += 1;
        }
        sqlx::query(
            "INSERT INTO activity (created_at, kind, user_id, profile_id, item_id, actor,
                                   summary, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(entry.created_at)
        .bind(&entry.kind)
        .bind(user_id)
        .bind(profile_id)
        .bind(item_id)
        .bind(&entry.actor)
        .bind(&entry.summary)
        .bind(&entry.detail)
        .execute(&mut *conn)
        .await
        .map_err(|e| format!("adding activity: {e}"))?;
        report.activity.created += 1;
    }
    if detached > 0 {
        report.notes.push(format!(
            "{detached} Activity entries name an account, profile or item that isn't imported; they are kept without that link, as for a deleted account."
        ));
    }
    Ok(())
}

/// What the admin should know that the counts don't say.
fn notes(bundle: &Bundle, report: &mut Report) {
    let locked = report
        .users
        .iter()
        .filter(|u| u.sign_in == SignIn::Locked)
        .count();
    if locked > 0 {
        report.notes.push(format!(
            "The bundle carries no password hashes: {locked} account(s) have no usable password until an admin sets one under Server → Users (D37). Profiles that had a PIN get one nobody knows; their owner can set a new PIN when editing the profile."
        ));
    }
    let conflicts = report
        .users
        .iter()
        .filter(|u| u.action == UserAction::Conflict)
        .count();
    if conflicts > 0 {
        report.notes.push(format!(
            "{conflicts} account(s) clash with a different account here and are skipped, with their profiles, grants, watch state and recent searches. Rename the account here, or in the bundle's server, and import again."
        ));
    }
    if report.items.created > 0 {
        report.notes.push(
            "New items arrive unavailable: they appear once a scan of their library finds their files. Scan the imported libraries after the import."
                .into(),
        );
    }
    let timed = bundle
        .catalog
        .items
        .iter()
        .flat_map(|i| i.segments.iter())
        .filter(|s| s.start_ms.is_some())
        .count();
    if timed > 0 {
        report.notes.push(format!(
            "{timed} intro/credits/recap segments are times into the old host's files; they are right only if this host has the same encodes."
        ));
    }
    let review = bundle
        .catalog
        .items
        .iter()
        .filter(|i| i.metadata_status == MetadataStatus::NeedsReview)
        .count();
    if review > 0 {
        report.notes.push(format!(
            "{review} item(s) need a metadata review and arrive without their list of candidates; the next metadata match rebuilds it."
        ));
    }
    if !bundle.catalog.items.is_empty() {
        report.notes.push(
            "Items match on their library and scanner key, which depends on the folders under each library root being laid out as on the old host; failing that, on a provider id."
                .into(),
        );
    }
    report.notes.push(
        "Audiobook progress and chapters are not part of format v1 (the audio proposal in IMPORT-EXPORT.md §9 is still open), so none is imported."
            .into(),
    );
}
