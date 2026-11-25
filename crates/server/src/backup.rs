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
//! - **Setting:** `PARLOUR_BACKUP_DIR` (an absolute path; unset means
//!   no backups) and `PARLOUR_BACKUP_KEEP` (how many nightly bundles
//!   to keep, default [`DEFAULT_KEEP`]). Environment-only, like every
//!   other setting (there is no settings table).
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

use chrono::{DateTime, Utc};
use uuid::Uuid;

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

/// Where nightly bundles go and how many are kept. `None` (no
/// `PARLOUR_BACKUP_DIR`) means nightly backups are off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSettings {
    pub dir: PathBuf,
    pub keep: u32,
}

impl BackupSettings {
    pub fn from_config(config: &crate::config::Config) -> Option<Self> {
        config.backup_dir.clone().map(|dir| Self {
            dir,
            keep: config.backup_keep,
        })
    }
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
/// off. Returns the new job's id.
pub async fn enqueue_nightly(
    pool: &parlour_db::PgPool,
    settings: Option<&BackupSettings>,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, sqlx::Error> {
    if settings.is_none() {
        return Ok(None);
    }
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
        // The flags are spelled out for whoever reads the job; the
        // worker forces both off for a nightly job whatever it says.
        serde_json::json!({
            "trigger": NIGHTLY,
            "include_hashes": false,
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
