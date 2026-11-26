//! XFER3: import a `parlour-export` v1 bundle (D14, D25, D37, D38;
//! spec `docs/IMPORT-EXPORT.md`, §12 for this task).
//!
//! Upload → parse → dry-run report → apply as a job:
//!
//! - `POST /api/v1/admin/import`: the bundle as the request body (a
//!   zip, at most [`MAX_UPLOAD_BYTES`]). It is streamed to
//!   `<PARLOUR_CACHE_DIR>/imports/<id>.zip`, opened with
//!   [`BundleReader`] (which checks the whole bundle and refuses an
//!   inconsistent one), and answered with its id and a dry-run
//!   [`Report`].
//! - `POST /api/v1/admin/import/{id}/dry-run`: the report again, for
//!   the server as it is now, with the admin's [`ImportOptions`]
//!   (library folders for this host in place of the old host's).
//! - `POST /api/v1/admin/import/{id}/apply`: queue an `import_bundle`
//!   job with those options; 202 with the job. The job runs at once;
//!   [`spawn_worker`] picks up one left queued.
//! - `DELETE /api/v1/admin/import/{id}`: remove an upload.
//! - `GET /api/v1/admin/import` (XFER8): the uploads waiting here, for
//!   Server → Import/Export.
//!
//! The dry run and the job are the same walk ([`apply::run`]) in a
//! transaction — rolled back for the dry run, committed by the job —
//! so the report is what the apply does. The job deletes the upload
//! once it is applied (it may hold password hashes), and records the
//! import in the Activity feed.

mod apply;

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use axum::body::{Body, HttpBody};
use axum::extract::{DefaultBodyLimit, Path as AxumPath, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

pub use apply::{
    Conflict, ImportOptions, LibraryAction, LibraryOutcome, PathCheck, Report, SignIn, Tally,
    UserAction, UserOutcome,
};

use crate::auth::AdminUser;
use crate::bundle::{Bundle, BundleReader};
use crate::error::AppError;
use crate::http::AppState;

/// The largest bundle an upload may be: 4 GiB, room for a household
/// server's documents plus its artwork. The documents themselves are
/// parsed in memory and bounded separately
/// ([`crate::bundle::MAX_DOCUMENT_BYTES`]).
pub const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Where uploaded bundles wait: `<cache dir>/imports`.
pub fn import_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("imports")
}

fn upload_path(import_dir: &Path, id: Uuid) -> PathBuf {
    import_dir.join(format!("{id}.zip"))
}

/// Where a job extracts the bundle's artwork before placing it.
fn staging_dir(import_dir: &Path, job_id: Uuid) -> PathBuf {
    import_dir.join(format!("{job_id}.artwork"))
}

type Opened = (BundleReader<BufReader<std::fs::File>>, Bundle);

/// Open an uploaded bundle (blocking: run it off the async threads).
fn open_bundle(path: &Path) -> Result<Opened, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("the upload can't be read: {e}"))?;
    BundleReader::open(BufReader::new(file)).map_err(|e| e.to_string())
}

async fn open_upload(import_dir: &Path, id: Uuid) -> Result<Bundle, AppError> {
    let path = upload_path(import_dir, id);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Err(AppError::NotFound("no such upload".into()));
    }
    tokio::task::spawn_blocking(move || open_bundle(&path).map(|(_, bundle)| bundle))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(AppError::BadRequest)
}

/// The dry run: the whole import, rolled back.
pub async fn dry_run(
    pool: &parlour_db::PgPool,
    bundle: &Bundle,
    options: &ImportOptions,
) -> Result<Report, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let report = apply::run(&mut tx, bundle, options, apply::Mode::DryRun, None).await;
    let _ = tx.rollback().await;
    report
}

// ------------------------------------------------------------ the job

/// Pull the bundle's artwork out to `staging`, each file checked
/// against its hash as it is copied ([`BundleReader::copy_artwork`]).
/// Only the names the reader lists are extracted. A file that fails
/// its check is left out (its image stays a provider reference) and
/// named in the returned errors.
fn stage_artwork(
    reader: &mut BundleReader<BufReader<std::fs::File>>,
    staging: &Path,
) -> Result<(BTreeMap<String, PathBuf>, Vec<String>), String> {
    let names: Vec<String> = reader.artwork_names().map(str::to_string).collect();
    let mut staged = BTreeMap::new();
    let mut errors = Vec::new();
    if names.is_empty() {
        return Ok((staged, errors));
    }
    std::fs::create_dir_all(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    for name in names {
        let Some(file_name) = Path::new(&name).file_name() else {
            continue;
        };
        let dest = staging.join(file_name);
        let copied = std::fs::File::create(&dest)
            .map_err(|e| e.to_string())
            .and_then(|mut out| {
                reader
                    .copy_artwork(&name, &mut out)
                    .map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())
            });
        match copied {
            Ok(()) => {
                staged.insert(name, dest);
            }
            Err(message) => {
                let _ = std::fs::remove_file(&dest);
                errors.push(format!("{name}: {message}"));
            }
        }
    }
    Ok((staged, errors))
}

/// The directories an import job works in.
#[derive(Debug, Clone)]
pub struct ImportDirs {
    /// Uploads and staged artwork (`<cache>/imports`).
    pub import_dir: PathBuf,
    /// The artwork cache imported files go into
    /// (`PARLOUR_ARTWORK_DIR`).
    pub artwork_dir: PathBuf,
}

async fn import(
    pool: &parlour_db::PgPool,
    job_id: Uuid,
    upload_id: Uuid,
    options: &ImportOptions,
    dirs: &ImportDirs,
) -> Result<Report, String> {
    let path = upload_path(&dirs.import_dir, upload_id);
    let staging = staging_dir(&dirs.import_dir, job_id);
    let (bundle, staged, errors) = {
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || {
            let (mut reader, bundle) = open_bundle(&path)?;
            let (staged, errors) = stage_artwork(&mut reader, &staging)?;
            Ok::<_, String>((bundle, staged, errors))
        })
        .await
        .map_err(|e| e.to_string())??
    };
    for message in &errors {
        let _ = parlour_jobs::add_error(
            pool,
            job_id,
            None,
            &format!("artwork left out of the import: {message}"),
        )
        .await;
    }
    let result = async {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let report = apply::run(
            &mut tx,
            &bundle,
            options,
            apply::Mode::Apply {
                staged: &staged,
                artwork_dir: &dirs.artwork_dir,
            },
            Some(apply::Progress { pool, job_id }),
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(report)
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

/// Run one claimed `import_bundle` job: apply the upload, record the
/// report, delete the upload, and note the import in the Activity
/// feed. A failed import changes nothing (its transaction is rolled
/// back) and keeps the upload for another try.
pub async fn run_import_job_claimed(
    pool: parlour_db::PgPool,
    job: parlour_jobs::Job,
    dirs: &ImportDirs,
) {
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
    let upload_id = job.payload["upload_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());
    let options =
        serde_json::from_value::<ImportOptions>(job.payload["options"].clone()).unwrap_or_default();
    let outcome = match upload_id {
        Some(upload_id) => import(&pool, job.id, upload_id, &options, dirs).await,
        None => Err("the job names no upload".to_string()),
    };
    match outcome {
        Ok(report) => {
            if let Some(upload_id) = upload_id {
                let _ = tokio::fs::remove_file(upload_path(&dirs.import_dir, upload_id)).await;
            }
            let count =
                |action: UserAction| report.users.iter().filter(|u| u.action == action).count();
            let detail = serde_json::json!({
                "job_id": job.id,
                "hashes_included": report.manifest.hashes_included,
                "users_created": count(UserAction::Create),
                "users_adopted": count(UserAction::Adopt),
                "user_conflicts": count(UserAction::Conflict),
                "items_created": report.items.created,
                "conflicts": report.conflicts_total,
            });
            tracing::info!(job_id = %job.id, conflicts = report.conflicts_total,
                items_created = report.items.created, "server import applied");
            let _ = parlour_jobs::set_result(
                &pool,
                job.id,
                serde_json::to_value(&report).unwrap_or(serde_json::Value::Null),
            )
            .await;
            let _ = parlour_jobs::complete(&pool, job.id).await;
            crate::activity::record(
                &pool,
                entry("import.bundle_applied", "Server bundle imported").detail(detail),
            )
            .await;
        }
        Err(message) => {
            tracing::warn!(job_id = %job.id, error = %message, "server import failed");
            let _ = parlour_jobs::fail(&pool, job.id, &message).await;
            crate::activity::record(
                &pool,
                entry("import.bundle_failed", "Server bundle import failed")
                    .detail(serde_json::json!({ "job_id": job.id })),
            )
            .await;
        }
    }
}

/// The worker for import jobs left queued (the request that queued
/// one normally runs it at once). Called once from `main`.
pub fn spawn_worker(pool: parlour_db::PgPool, dirs: ImportDirs) {
    tokio::spawn(async move {
        loop {
            match parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::ImportBundle).await {
                Ok(Some(job)) => run_import_job_claimed(pool.clone(), job, &dirs).await,
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(3)).await,
                Err(e) => {
                    tracing::warn!(error = %e, "import job claim failed");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    });
}

// ------------------------------------------------------------ endpoints

#[derive(Debug, Serialize)]
struct Uploaded {
    id: Uuid,
    bytes: u64,
    report: Report,
}

/// Stream the request body to `dest`, refusing more than `limit`
/// bytes. Returns the size.
async fn save_body(mut body: Body, dest: &Path, limit: u64) -> Result<u64, AppError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| AppError::Internal(format!("{}: {e}", dest.display())))?;
    let mut size = 0u64;
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame =
            frame.map_err(|e| AppError::BadRequest(format!("the upload broke off: {e}")))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        size += data.len() as u64;
        if size > limit {
            return Err(AppError::BadRequest(format!(
                "the upload is larger than {limit} bytes, the most this server takes"
            )));
        }
        file.write_all(&data)
            .await
            .map_err(|e| AppError::Internal(format!("{}: {e}", dest.display())))?;
    }
    file.flush()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    file.sync_all()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(size)
}

async fn upload(
    State(app): State<AppState>,
    _admin: AdminUser,
    body: Body,
) -> Result<(StatusCode, Json<Uploaded>), AppError> {
    tokio::fs::create_dir_all(&app.import_dir)
        .await
        .map_err(|e| AppError::Internal(format!("{}: {e}", app.import_dir.display())))?;
    let id = Uuid::new_v4();
    let dest = upload_path(&app.import_dir, id);
    let part = dest.with_extension("zip.part");
    let saved = async {
        let bytes = save_body(body, &part, MAX_UPLOAD_BYTES).await?;
        let checked = part.clone();
        let bundle = tokio::task::spawn_blocking(move || open_bundle(&checked))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
            .map_err(|message| {
                AppError::BadRequest(format!("not an importable bundle: {message}"))
            })?
            .1;
        tokio::fs::rename(&part, &dest)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        Ok::<_, AppError>((bytes, bundle))
    }
    .await;
    let (bytes, bundle) = match saved {
        Ok(saved) => saved,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
    };
    let report = dry_run(&app.pool, &bundle, &ImportOptions::default())
        .await
        .map_err(AppError::Internal)?;
    Ok((StatusCode::CREATED, Json(Uploaded { id, bytes, report })))
}

/// One upload waiting on the server for its dry run and apply.
#[derive(Debug, Serialize)]
struct Pending {
    id: Uuid,
    bytes: u64,
    uploaded_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The upload id in a file name, if it is exactly `<uuid>.zip` (not a
/// `.part` still arriving, nor a job's staging folder).
fn pending_id(name: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(name.strip_suffix(".zip")?).ok()?;
    (name == format!("{id}.zip")).then_some(id)
}

/// `GET /api/v1/admin/import` (XFER8): the uploads waiting here,
/// newest first, so Server → Import/Export can pick one up again or
/// delete it (an upload may hold hashes).
async fn list_uploads(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<Pending>>, AppError> {
    let dir = app.import_dir.clone();
    let mut pending = tokio::task::spawn_blocking(move || {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().and_then(pending_id) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            out.push(Pending {
                id,
                bytes: meta.len(),
                uploaded_at: meta
                    .modified()
                    .ok()
                    .map(chrono::DateTime::<chrono::Utc>::from),
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(AppError::Internal)?;
    pending.sort_by(|a, b| b.uploaded_at.cmp(&a.uploaded_at).then(a.id.cmp(&b.id)));
    Ok(Json(pending))
}

fn upload_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such upload".into()))
}

async fn dry_run_upload(
    State(app): State<AppState>,
    _admin: AdminUser,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<ImportOptions>>,
) -> Result<Json<Report>, AppError> {
    let id = upload_id(&id)?;
    let options = body.map(|Json(o)| o).unwrap_or_default();
    let bundle = open_upload(&app.import_dir, id).await?;
    options.check(&bundle).map_err(AppError::BadRequest)?;
    let report = dry_run(&app.pool, &bundle, &options)
        .await
        .map_err(AppError::Internal)?;
    Ok(Json(report))
}

async fn apply_upload(
    State(app): State<AppState>,
    admin: AdminUser,
    AxumPath(id): AxumPath<String>,
    body: Option<Json<ImportOptions>>,
) -> Result<(StatusCode, Json<parlour_jobs::Job>), AppError> {
    let id = upload_id(&id)?;
    let options = body.map(|Json(o)| o).unwrap_or_default();
    let bundle = open_upload(&app.import_dir, id).await?;
    options.check(&bundle).map_err(AppError::BadRequest)?;
    let job_id = parlour_jobs::enqueue(
        &app.pool,
        parlour_jobs::JobKind::ImportBundle,
        None,
        serde_json::json!({
            "trigger": "manual",
            "upload_id": id,
            "options": options,
            "requested_by": { "user_id": admin.0.user_id, "username": admin.0.username },
        }),
    )
    .await
    .map_err(AppError::from)?;
    let pool = app.pool.clone();
    let dirs = ImportDirs {
        import_dir: app.import_dir.clone(),
        artwork_dir: app.metadata.artwork_dir.clone(),
    };
    tokio::spawn(async move {
        if parlour_jobs::claim_by_id(&pool, job_id)
            .await
            .unwrap_or(false)
        {
            if let Ok(Some(job)) = parlour_jobs::get(&pool, job_id).await {
                run_import_job_claimed(pool, job, &dirs).await;
            }
        }
    });
    let job = parlour_jobs::get(&app.pool, job_id)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::Internal("job vanished after enqueue".into()))?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

async fn delete_upload(
    State(app): State<AppState>,
    _admin: AdminUser,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let id = upload_id(&id)?;
    match tokio::fs::remove_file(upload_path(&app.import_dir, id)).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(AppError::NotFound("no such upload".into()))
        }
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/admin/import",
            // The upload streams to disk under its own limit.
            post(upload)
                .layer(DefaultBodyLimit::disable())
                .get(list_uploads),
        )
        .route("/api/v1/admin/import/{id}/dry-run", post(dry_run_upload))
        .route("/api/v1/admin/import/{id}/apply", post(apply_upload))
        .route("/api/v1/admin/import/{id}", delete(delete_upload))
}

#[cfg(test)]
mod tests;
