//! XFER7: automatic backups — a nightly `parlour-export` v1 bundle
//! written to a folder the admin configures, with retention
//! (D14, D25; spec `docs/IMPORT-EXPORT.md` §15). Restore is XFER3's
//! import.
//!
//! There is no second export here. The nightly tick
//! (`catalog::nightly_tick`) queues an ordinary `export_bundle` job
//! whose payload says `"trigger": "nightly"`; the export worker runs
//! it through the same code as a manual export (`export::export`),
//! with hashes and artwork off, and writes the zip straight into the
//! backup folder under a dated name. Only after that bundle is in
//! place does retention delete the oldest nightly bundles past the
//! configured count.
//!
//! - **Setting (D39, XFER8):** set up in the web UI (Server →
//!   Import/Export) and stored in `backup_settings`: the folder (an
//!   absolute path; none means no backups), how many nightly bundles
//!   to keep, and whether they carry password and PIN hashes (off
//!   unless the admin turns it on past the export's warning). Until an
//!   admin saves it, the environment is the bootstrap default:
//!   `PARLOUR_BACKUP_DIR` (unset means no backups) and
//!   `PARLOUR_BACKUP_KEEP` (default [`DEFAULT_KEEP`]), never with
//!   hashes. Read fresh wherever it is used ([`effective`]), so a
//!   saved change takes effect without a restart.
//! - **Name:** `parlour-backup-YYYYMMDD-HHMMSS.zip`, the export's own
//!   UTC time ([`file_name`]). Retention only ever touches names of
//!   exactly that shape ([`is_backup_name`]), so nothing else in the
//!   folder is at risk.
//! - **A bad folder** (missing, not a folder, not writable) fails the
//!   job with the path in its message. Parlour never creates the
//!   folder: a missing folder is usually an unmounted disk, and
//!   creating it would fill the disk underneath the mount point. The
//!   next night queues a new job and tries again.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

/// Nightly bundles kept when `PARLOUR_BACKUP_KEEP` is unset.
pub const DEFAULT_KEEP: u32 = 7;
/// The most `PARLOUR_BACKUP_KEEP` may ask for (a year of nights).
pub const MAX_KEEP: u32 = 365;

/// The `trigger` a nightly backup job's payload carries.
pub const NIGHTLY: &str = "nightly";

const PREFIX: &str = "parlour-backup-";
const SUFFIX: &str = ".zip";
/// What `export::write_bundle` names the file while it is written.
const PART_SUFFIX: &str = ".zip.part";

/// Where nightly bundles go, how many are kept, and whether they
/// carry hashes. `None` (no folder) means nightly backups are off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSettings {
    pub dir: PathBuf,
    pub keep: u32,
    /// D14's confirm for the standing job (D39): only an admin turns
    /// it on, in the web UI, past the export's warning. The
    /// environment's default never does.
    pub include_hashes: bool,
}

impl BackupSettings {
    /// The environment's bootstrap default (`PARLOUR_BACKUP_DIR`,
    /// `PARLOUR_BACKUP_KEEP`), used until an admin saves the setting.
    pub fn from_config(config: &crate::config::Config) -> Option<Self> {
        config.backup_dir.clone().map(|dir| Self {
            dir,
            keep: config.backup_keep,
            include_hashes: false,
        })
    }
}

/// The setting as an admin saved it on Server → Import/Export (D39).
/// `folder: None` is "off", which overrides the environment too.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SavedSettings {
    pub folder: Option<String>,
    pub keep: i32,
    pub include_hashes: bool,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
}

/// The saved setting, if an admin has saved one.
pub async fn saved(pool: &parlour_db::PgPool) -> Result<Option<SavedSettings>, sqlx::Error> {
    sqlx::query_as::<_, SavedSettings>(
        "SELECT b.folder, b.keep, b.include_hashes, b.updated_at, u.username AS updated_by
         FROM backup_settings b LEFT JOIN users u ON u.id = b.updated_by",
    )
    .fetch_optional(pool)
    .await
}

/// What applies: the saved setting when there is one, otherwise the
/// environment's default (`env`). Read wherever backups are used, so
/// a saved change needs no restart.
pub fn resolve(
    saved: Option<&SavedSettings>,
    env: Option<&BackupSettings>,
) -> Option<BackupSettings> {
    match saved {
        Some(s) => s.folder.as_ref().map(|folder| BackupSettings {
            dir: PathBuf::from(folder),
            keep: u32::try_from(s.keep).unwrap_or(DEFAULT_KEEP),
            include_hashes: s.include_hashes,
        }),
        None => env.cloned(),
    }
}

/// [`resolve`] against the database.
pub async fn effective(
    pool: &parlour_db::PgPool,
    env: Option<&BackupSettings>,
) -> Result<Option<BackupSettings>, sqlx::Error> {
    Ok(resolve(saved(pool).await?.as_ref(), env))
}

/// The file a nightly bundle is written as: its export time in UTC,
/// fixed width, so names sort in time order.
pub fn file_name(exported_at: DateTime<Utc>) -> String {
    format!("{PREFIX}{}{SUFFIX}", exported_at.format("%Y%m%d-%H%M%S"))
}

/// The stamp inside a backup name (`YYYYMMDD-HHMMSS`), if `name` is
/// exactly `<prefix><stamp><suffix>`.
fn stamp<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let stamp = name.strip_prefix(PREFIX)?.strip_suffix(suffix)?;
    let b = stamp.as_bytes();
    let shaped = b.len() == 15
        && b[8] == b'-'
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[9..].iter().all(u8::is_ascii_digit);
    shaped.then_some(stamp)
}

/// Is `name` a nightly bundle this server wrote? Exactly
/// `parlour-backup-` + 8 digits + `-` + 6 digits + `.zip`.
pub fn is_backup_name(name: &str) -> bool {
    stamp(name, SUFFIX).is_some()
}

/// What retention deletes from a folder holding `names`: every
/// nightly bundle past the newest `keep`, plus any half-written
/// `<backup name>.part` other than `current_part`'s (a crash's
/// leftover; only one backup job runs at a time). Anything not named
/// like a backup is never listed.
pub fn retention_victims<I, S>(names: I, keep: u32) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut bundles = Vec::new();
    let mut parts = Vec::new();
    for name in names {
        let name = name.as_ref();
        if let Some(stamp) = stamp(name, SUFFIX) {
            bundles.push((stamp.to_string(), name.to_string()));
        } else if stamp(name, PART_SUFFIX).is_some() {
            parts.push(name.to_string());
        }
    }
    // Newest first; the stamp is fixed width, so text order is time
    // order.
    bundles.sort_by(|a, b| b.0.cmp(&a.0));
    let mut victims: Vec<String> = bundles
        .into_iter()
        .skip(keep.max(1) as usize)
        .map(|(_, name)| name)
        .collect();
    parts.sort();
    victims.extend(parts);
    victims
}

/// Check the backup folder before an export is spent on it: it must
/// exist, be a folder, and be writable by this process. The message
/// names the path.
pub fn check_folder(dir: &Path) -> Result<(), String> {
    let shown = dir.display();
    match std::fs::metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let why = "Parlour doesn't create it: is its disk mounted?";
            return Err(format!("the backup folder {shown} doesn't exist ({why})"));
        }
        Err(e) => {
            return Err(format!(
                "the backup folder {} can't be read: {e}",
                dir.display()
            ))
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(format!("the backup path {} is not a folder", dir.display()))
        }
        Ok(_) => {}
    }
    crate::libraries::write_access(dir)
        .map_err(|e| format!("the backup folder {} can't be written: {e}", dir.display()))
}

/// What one retention pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retention {
    /// Nightly bundles (and stale `.part` files) deleted.
    pub removed: Vec<String>,
    /// (name, reason) for each one that couldn't be deleted.
    pub errors: Vec<(String, String)>,
}

/// Delete what [`retention_victims`] lists in `dir`. Run only after
/// the night's bundle is in place, so a failed night never costs a
/// good bundle. A file that can't be deleted is reported, not fatal:
/// the bundle just written is still a good backup.
pub fn apply_retention(dir: &Path, keep: u32) -> Result<Retention, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("listing {}: {e}", dir.display()))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("listing {}: {e}", dir.display()))?;
        // Only plain files: a folder that happens to carry a backup's
        // name is left alone.
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    let mut out = Retention::default();
    for name in retention_victims(&names, keep) {
        match std::fs::remove_file(dir.join(&name)) {
            Ok(()) => out.removed.push(name),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.errors.push((name, e.to_string())),
        }
    }
    Ok(out)
}

/// Queue tonight's backup unless one was already queued since the
/// start of `now`'s UTC day (failed or not: a failed night is retried
/// the next night, not every tick). Does nothing when backups are
/// off — by the saved setting, or, with none saved, by `env` (the
/// environment's default). Returns the new job's id.
pub async fn enqueue_nightly(
    pool: &parlour_db::PgPool,
    env: Option<&BackupSettings>,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let Some(settings) = effective(pool, env).await? else {
        return Ok(None);
    };
    let start_of_day = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc();
    if parlour_jobs::exists_since(
        pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        start_of_day,
        NIGHTLY,
    )
    .await?
    {
        return Ok(None);
    }
    parlour_jobs::enqueue(
        pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        // The flags are spelled out for whoever reads the job. The
        // worker never adds artwork, and adds hashes only when this
        // says so *and* the setting still does when it runs.
        serde_json::json!({
            "trigger": NIGHTLY,
            "include_hashes": settings.include_hashes,
            "include_artwork": false,
        }),
    )
    .await
    .map(Some)
}

/// Mark the jobs whose bundles retention just removed, so the Jobs
/// page stops offering their download.
pub async fn mark_removed(pool: &parlour_db::PgPool, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let result = sqlx::query(
        "UPDATE jobs
         SET result = result || '{\"deleted\": true, \"removed_by_retention\": true}'::jsonb
         WHERE kind = 'export_bundle'
           AND payload ->> 'trigger' = 'nightly'
           AND result ->> 'file_name' = ANY($1)",
    )
    .bind(names)
    .execute(pool)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "marking retired backups failed");
    }
}

// ------------------------------------------------------------ the admin setting (D39)

/// Where the setting in force comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// An admin saved it on Server → Import/Export.
    Saved,
    /// Nothing saved yet: the environment's bootstrap default.
    Environment,
}

/// The environment's bootstrap default, shown beside the saved one.
#[derive(Debug, Clone, Serialize)]
struct EnvironmentDefault {
    folder: Option<String>,
    keep: u32,
}

/// `GET /api/v1/admin/backup`: the setting in force, where it comes
/// from, and whether its folder can take a bundle right now.
#[derive(Debug, Clone, Serialize)]
struct SettingsView {
    source: Source,
    /// `null` = nightly backups are off.
    folder: Option<String>,
    keep: u32,
    include_hashes: bool,
    /// Why the folder can't take a bundle right now (the nightly job's
    /// own check, [`check_folder`]); `null` when it can, or when off.
    folder_problem: Option<String>,
    updated_at: Option<DateTime<Utc>>,
    updated_by: Option<String>,
    environment: EnvironmentDefault,
}

async fn view(app: &AppState) -> Result<SettingsView, AppError> {
    let saved = saved(&app.pool).await.map_err(AppError::from)?;
    let current = resolve(saved.as_ref(), app.backup.as_ref());
    let folder_problem = match &current {
        Some(s) => {
            let dir = s.dir.clone();
            tokio::task::spawn_blocking(move || check_folder(&dir))
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?
                .err()
        }
        None => None,
    };
    let environment = EnvironmentDefault {
        folder: app.backup.as_ref().map(|b| b.dir.display().to_string()),
        keep: app.backup.as_ref().map(|b| b.keep).unwrap_or(DEFAULT_KEEP),
    };
    // Saved: the admin's values (a blank folder is "off", and keep and
    // hashes are shown as saved). Otherwise the environment's, which
    // never carry hashes.
    Ok(match saved {
        Some(s) => SettingsView {
            source: Source::Saved,
            folder: s.folder,
            keep: u32::try_from(s.keep).unwrap_or(DEFAULT_KEEP),
            include_hashes: s.include_hashes,
            folder_problem,
            updated_at: Some(s.updated_at),
            updated_by: s.updated_by,
            environment,
        },
        None => SettingsView {
            source: Source::Environment,
            folder: environment.folder.clone(),
            keep: environment.keep,
            include_hashes: false,
            folder_problem,
            updated_at: None,
            updated_by: None,
            environment,
        },
    })
}

async fn get_settings(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<SettingsView>, AppError> {
    Ok(Json(view(&app).await?))
}

/// `PUT /api/v1/admin/backup`: what the admin saves. Strict.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSettings {
    /// The folder, as the server sees it; `null` or blank = off.
    #[serde(default)]
    folder: Option<String>,
    keep: u32,
    /// D14's confirm for the standing job; off unless sent as `true`.
    #[serde(default)]
    include_hashes: bool,
}

/// The folder an admin typed, checked the way `PARLOUR_BACKUP_DIR` is:
/// blank is "off"; otherwise an absolute path.
fn clean_folder(folder: Option<&str>) -> Result<Option<String>, AppError> {
    let Some(folder) = folder.map(str::trim).filter(|f| !f.is_empty()) else {
        return Ok(None);
    };
    if !folder.starts_with('/') {
        return Err(AppError::BadRequest(
            "the backup folder must be an absolute path as the server sees it (in the container, e.g. /backups)".into(),
        ));
    }
    if folder.len() > 4096 || folder.contains('\0') {
        return Err(AppError::BadRequest(
            "that is not a usable folder path".into(),
        ));
    }
    Ok(Some(folder.to_string()))
}

async fn save_settings(
    State(app): State<AppState>,
    admin: AdminUser,
    Json(body): Json<SaveSettings>,
) -> Result<Json<SettingsView>, AppError> {
    let folder = clean_folder(body.folder.as_deref())?;
    if !(1..=MAX_KEEP).contains(&body.keep) {
        return Err(AppError::BadRequest(format!(
            "keep must be between 1 and {MAX_KEEP}"
        )));
    }
    let before = saved(&app.pool).await.map_err(AppError::from)?;
    let before = resolve(before.as_ref(), app.backup.as_ref());
    sqlx::query(
        "INSERT INTO backup_settings (id, folder, keep, include_hashes, updated_at, updated_by)
         VALUES (true, $1, $2, $3, now(), $4)
         ON CONFLICT (id) DO UPDATE
             SET folder = EXCLUDED.folder, keep = EXCLUDED.keep,
                 include_hashes = EXCLUDED.include_hashes,
                 updated_at = now(), updated_by = EXCLUDED.updated_by",
    )
    .bind(&folder)
    .bind(body.keep as i32)
    .bind(body.include_hashes)
    .bind(admin.0.user_id)
    .execute(&app.pool)
    .await
    .map_err(AppError::from)?;
    let summary = match (&folder, body.include_hashes) {
        (None, _) => "Nightly backups turned off",
        (Some(_), true) => "Nightly backups set, with password and PIN hashes",
        (Some(_), false) => "Nightly backups set",
    };
    crate::activity::record(
        &app.pool,
        crate::activity::Entry::by_user(
            "export.backup_settings",
            admin.0.user_id,
            &admin.0.username,
            summary,
        )
        .detail(serde_json::json!({
            "folder": folder,
            "keep": body.keep,
            "include_hashes": body.include_hashes,
            "hashes_before": before.as_ref().is_some_and(|b| b.include_hashes),
        })),
    )
    .await;
    Ok(Json(view(&app).await?))
}

/// `DELETE /api/v1/admin/backup`: forget the saved setting, so the
/// environment's default applies again.
async fn reset_settings(
    State(app): State<AppState>,
    admin: AdminUser,
) -> Result<Json<SettingsView>, AppError> {
    let done = sqlx::query("DELETE FROM backup_settings")
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() > 0 {
        crate::activity::record(
            &app.pool,
            crate::activity::Entry::by_user(
                "export.backup_settings",
                admin.0.user_id,
                &admin.0.username,
                "Nightly backup settings reset to the server's environment",
            )
            .detail(serde_json::json!({ "reset": true })),
        )
        .await;
    }
    Ok(Json(view(&app).await?))
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/admin/backup",
        get(get_settings).put(save_settings).delete(reset_settings),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A fresh folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("parlour-xfer7-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_carry_the_utc_export_time() {
        let at = Utc.with_ymd_and_hms(2026, 11, 25, 7, 30, 5).unwrap();
        assert_eq!(file_name(at), "parlour-backup-20261125-073005.zip");
        assert!(is_backup_name(&file_name(at)));
    }

    #[test]
    fn only_exact_backup_names_count() {
        for good in [
            "parlour-backup-20261125-073005.zip",
            "parlour-backup-00000000-000000.zip",
        ] {
            assert!(is_backup_name(good), "{good}");
        }
        for bad in [
            "parlour-export-20261125-073005.zip",
            "parlour-backup-20261125-073005.zip.part",
            "parlour-backup-20261125-07300.zip",
            "parlour-backup-20261125_073005.zip",
            "parlour-backup-2026112a-073005.zip",
            "parlour-backup-20261125-073005.ZIP",
            "xparlour-backup-20261125-073005.zip",
            "parlour-backup-20261125-073005.zip.bak",
            "parlour-backup-.zip",
            "notes.txt",
        ] {
            assert!(!is_backup_name(bad), "{bad}");
        }
    }

    #[test]
    fn retention_keeps_the_newest_by_name_and_ignores_strangers() {
        let names = [
            "parlour-backup-20261120-073000.zip",
            "parlour-backup-20261124-073000.zip",
            "parlour-backup-20261122-073000.zip",
            "parlour-backup-20261123-073000.zip",
            "parlour-backup-20261121-073000.zip",
            "parlour-export-20261101-120000.zip",
            "README.txt",
            "parlour-backup-20261119-073000.zip.part",
        ];
        assert_eq!(
            retention_victims(names, 3),
            vec![
                "parlour-backup-20261121-073000.zip".to_string(),
                "parlour-backup-20261120-073000.zip".to_string(),
                "parlour-backup-20261119-073000.zip.part".to_string(),
            ]
        );
        // Fewer bundles than `keep`: only the stale part goes.
        assert_eq!(
            retention_victims(names, 30),
            vec!["parlour-backup-20261119-073000.zip.part".to_string()]
        );
        // `keep` never drops below one (the bundle just written).
        assert_eq!(retention_victims(names, 0).len(), 4 + 1);
    }

    #[test]
    fn retention_on_disk_deletes_only_backups_past_keep() {
        let dir = TempDir::new();
        for name in [
            "parlour-backup-20261120-073000.zip",
            "parlour-backup-20261121-073000.zip",
            "parlour-backup-20261122-073000.zip",
            "parlour-export-20261101-120000.zip",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        // A folder with a backup's name is never touched.
        std::fs::create_dir(dir.path().join("parlour-backup-20261101-000000.zip")).unwrap();
        let done = apply_retention(dir.path(), 2).unwrap();
        assert_eq!(done.removed, vec!["parlour-backup-20261120-073000.zip"]);
        assert!(done.errors.is_empty());
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "notes.txt",
                "parlour-backup-20261101-000000.zip",
                "parlour-backup-20261121-073000.zip",
                "parlour-backup-20261122-073000.zip",
                "parlour-export-20261101-120000.zip",
            ]
        );
    }

    fn saved(folder: Option<&str>, keep: i32, include_hashes: bool) -> SavedSettings {
        SavedSettings {
            folder: folder.map(str::to_string),
            keep,
            include_hashes,
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    #[test]
    fn the_saved_setting_wins_and_the_environment_is_only_the_default() {
        let env = BackupSettings {
            dir: PathBuf::from("/env-backups"),
            keep: 9,
            include_hashes: false,
        };
        // Nothing saved: the environment's default, or off.
        assert_eq!(resolve(None, Some(&env)), Some(env.clone()));
        assert_eq!(resolve(None, None), None);
        // Saved with a folder: that, whatever the environment says.
        let ui = saved(Some("/srv/parlour-backups"), 14, true);
        let want = Some(BackupSettings {
            dir: PathBuf::from("/srv/parlour-backups"),
            keep: 14,
            include_hashes: true,
        });
        assert_eq!(resolve(Some(&ui), Some(&env)), want);
        assert_eq!(resolve(Some(&ui), None), want);
        // Saved off: off, even with the environment set.
        assert_eq!(resolve(Some(&saved(None, 7, false)), Some(&env)), None);
    }

    #[test]
    fn the_environment_never_turns_hashes_on() {
        let mut vars = std::collections::HashMap::new();
        vars.insert(
            "PARLOUR_DATABASE_URL".to_string(),
            "postgres://x@localhost/x".to_string(),
        );
        vars.insert("PARLOUR_BACKUP_DIR".to_string(), "/backups".to_string());
        let config = crate::config::Config::from_vars(&vars).unwrap();
        let env = BackupSettings::from_config(&config).unwrap();
        assert!(!env.include_hashes);
    }

    #[test]
    fn a_typed_folder_is_blank_or_absolute() {
        assert_eq!(clean_folder(None).unwrap(), None);
        assert_eq!(clean_folder(Some("   ")).unwrap(), None);
        assert_eq!(
            clean_folder(Some(" /backups ")).unwrap(),
            Some("/backups".to_string())
        );
        assert!(clean_folder(Some("backups")).is_err());
        assert!(clean_folder(Some("./backups")).is_err());
        assert!(clean_folder(Some("/back\0ups")).is_err());
    }

    #[test]
    fn a_bad_folder_is_named_in_the_message() {
        let dir = TempDir::new();
        let missing = dir.path().join("not-mounted");
        let err = check_folder(&missing).unwrap_err();
        assert!(err.contains(&missing.display().to_string()), "{err}");
        assert!(err.contains("doesn't exist"), "{err}");
        assert!(!missing.exists(), "the folder is never created");

        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        let err = check_folder(&file).unwrap_err();
        assert!(err.contains("is not a folder"), "{err}");

        assert_eq!(check_folder(dir.path()), Ok(()));
    }
}
