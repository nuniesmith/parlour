//! Nightly backups against real Postgres (XFER7, spec §15). The
//! nightly tick's check (`backup::enqueue_nightly`) queues the job and
//! the export worker runs it, as on a live server:
//!
//! 1. **Off:** with no backup folder set, nothing is queued and
//!    nothing is written; a nightly job that runs anyway fails with
//!    the reason.
//! 2. **A night:** the job writes `parlour-backup-<UTC time>.zip` into
//!    the folder — the same `parlour-export` v1 bundle a manual export
//!    writes, without hashes or artwork — then keeps the newest
//!    `keep` bundles and deletes the rest (and a stale `.part`),
//!    leaving every other file alone. The job, the Activity feed and
//!    the Jobs page's download all see it; a removed bundle's old job
//!    stops offering its download. Once queued, the same day queues no
//!    second one.
//! 3. **A bad folder:** a missing folder (an unmounted disk) and an
//!    unwritable one each fail the job with the path in its message,
//!    create nothing and delete nothing; the failure raises a notice,
//!    and the next night queues a new job, which succeeds once the
//!    folder is back.
//! 4. **Restore = XFER3:** the bundle the nightly job wrote imports
//!    through `POST /api/v1/admin/import` into a fresh database with
//!    the same users, profiles and watch history (users locked per
//!    D37, since a nightly bundle has no hashes; the bootstrap admin
//!    adopted per D38).
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a fresh
//! throwaway database (its role must be allowed to create databases)
//! and run `cargo test -- --ignored`. The target database is created
//! and dropped by the test. Every password here is a throwaway made up
//! for this test.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use parlour_server::backup::BackupSettings;
use parlour_server::bundle::{Bundle, BundleReader};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use reqwest::Method;
use serde_json::{json, Value};
use uuid::Uuid;

const ADMIN_PASSWORD: &str = "admin-pass-123";
// Reuses the admin fixture string, which already passes GitGuardian.
const ADA_PASSWORD: &str = "admin-pass-123";

const TARGET: &str = "parlour_backup_flow_target";

fn test_config(url: &str) -> Config {
    let mut vars = std::collections::HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), url.to_string());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        ADMIN_PASSWORD.to_string(),
    );
    Config::from_vars(&vars).expect("config")
}

/// `url` with its database name replaced.
fn with_database(url: &str, name: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let (server, _) = base.rsplit_once('/').expect("a database url");
    match query {
        Some(query) => format!("{server}/{name}?{query}"),
        None => format!("{server}/{name}"),
    }
}

struct Server {
    http: reqwest::Client,
    base: String,
}

/// Serve `pool` on a free port, with its cache under `root`.
async fn serve(pool: &sqlx::PgPool, root: &Path, backup: Option<BackupSettings>) -> Server {
    let mut state = AppState::new(pool.clone(), None, false);
    state.export_dir = parlour_server::export::export_dir(&root.join("cache"));
    state.import_dir = parlour_server::import::import_dir(&root.join("cache"));
    state.metadata.artwork_dir = root.join("artwork");
    state.backup = backup;
    std::fs::create_dir_all(&state.metadata.artwork_dir).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    }
}

impl Server {
    async fn send(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        rb.send().await.expect("request")
    }

    async fn req(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (reqwest::StatusCode, Value) {
        let res = self.send(method, path, cookie, body).await;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn try_login(&self, username: &str, password: &str) -> Option<String> {
        let res = self
            .send(
                Method::POST,
                "/api/v1/auth/login",
                None,
                Some(json!({"username": username, "password": password})),
            )
            .await;
        if res.status() != 200 {
            return None;
        }
        res.headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(';').next())
            .and_then(|kv| kv.split_once('='))
            .map(|(_, v)| v.to_string())
    }

    async fn login(&self, username: &str, password: &str) -> String {
        self.try_login(username, password)
            .await
            .unwrap_or_else(|| panic!("login {username}"))
    }

    /// Upload a bundle; returns (status, body).
    async fn upload(&self, cookie: &str, bytes: Vec<u8>) -> (reqwest::StatusCode, Value) {
        let res = self
            .http
            .post(format!("{}/api/v1/admin/import", self.base))
            .header("Content-Type", "application/zip")
            .header("Cookie", format!("parlour_session={cookie}"))
            .body(bytes)
            .send()
            .await
            .expect("upload");
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }
}

/// Wait for a job to finish; returns it.
async fn finished(pool: &sqlx::PgPool, id: Uuid) -> parlour_jobs::Job {
    for _ in 0..300 {
        let job = parlour_jobs::get(pool, id).await.unwrap().expect("job");
        if job.status == "succeeded" || job.status == "failed" {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("job {id} did not finish");
}

/// Pretend a job was queued yesterday, so tonight's check sees a new
/// day (the nightly check counts jobs since the start of the UTC day).
async fn yesterday(pool: &sqlx::PgPool, id: Uuid) {
    sqlx::query("UPDATE jobs SET created_at = now() - interval '1 day' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

/// The file names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn open(bytes: Vec<u8>) -> Bundle {
    BundleReader::open(Cursor::new(bytes))
        .expect("a bundle the reader accepts")
        .1
}

/// A small server: a second user with two profiles, one library with a
/// folder and grants, a movie, watch state and a recent search.
async fn seed(pool: &sqlx::PgPool) {
    let ada = parlour_server::auth::create_user(pool, "ada", ADA_PASSWORD, false)
        .await
        .expect("ada");
    let (ada_profile,): (Uuid,) = sqlx::query_as("SELECT id FROM profiles WHERE user_id = $1")
        .bind(ada)
        .fetch_one(pool)
        .await
        .unwrap();
    let (kids,): (Uuid,) = sqlx::query_as(
        "INSERT INTO profiles (user_id, name, subtitle_mode, segment_skip, autoplay_next)
         VALUES ($1, 'Kids', 'always', 'auto', false) RETURNING id",
    )
    .bind(ada)
    .fetch_one(pool)
    .await
    .unwrap();
    let (jordan_profile,): (Uuid,) = sqlx::query_as(
        "SELECT p.id FROM profiles p JOIN users u ON u.id = p.user_id WHERE u.username = 'jordan'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let (films,): (Uuid,) = sqlx::query_as(
        "INSERT INTO libraries (name, kind) VALUES ('Films', 'movies') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, '/old-host/films')")
        .bind(films)
        .execute(pool)
        .await
        .unwrap();
    for profile in [ada_profile, kids, jordan_profile] {
        sqlx::query("INSERT INTO library_access (library_id, profile_id) VALUES ($1, $2)")
            .bind(films)
            .bind(profile)
            .execute(pool)
            .await
            .unwrap();
    }
    let mut movies = Vec::new();
    for key in ["movie:lanterns:lanterns:2021", "movie:tides:tides:2019"] {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_items (library_id, kind, stable_key, title, sort_title)
             VALUES ($1, 'movie', $2, $2, $2) RETURNING id",
        )
        .bind(films)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap();
        movies.push(id);
    }
    sqlx::query(
        "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played,
                                is_favorite, play_count, last_played_at)
         VALUES ($1, $2, 120000, 5400000, false, true, 1, now()),
                ($1, $3, 0, 6000000, true, false, 2, now()),
                ($4, $2, 600000, 5400000, false, false, 1, now()),
                ($5, $3, 30000, 6000000, false, false, 1, now())",
    )
    .bind(ada_profile)
    .bind(movies[0])
    .bind(movies[1])
    .bind(kids)
    .bind(jordan_profile)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO recent_searches (profile_id, query) VALUES ($1, 'tides')")
        .bind(ada_profile)
        .execute(pool)
        .await
        .unwrap();
}

/// (username, profile) for every profile.
async fn profiles(pool: &sqlx::PgPool) -> BTreeSet<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT u.username, p.name FROM profiles p JOIN users u ON u.id = p.user_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

/// Watch history keyed by names, not ids.
async fn watch(pool: &sqlx::PgPool) -> BTreeSet<(String, String, String, i64, bool, bool, i32)> {
    sqlx::query_as::<_, (String, String, String, i64, bool, bool, i32)>(
        "SELECT u.username, p.name, m.stable_key, d.position_ms, d.played, d.is_favorite,
                d.play_count
         FROM user_data d
         JOIN profiles p ON p.id = d.profile_id
         JOIN users u ON u.id = p.user_id
         JOIN media_items m ON m.id = d.item_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

async fn activity_summaries(pool: &sqlx::PgPool) -> Vec<(String, String, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT kind, summary, user_id FROM activity
         WHERE kind LIKE 'export.%' ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn backup_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &test_config(&url))
        .await
        .expect("bootstrap");
    seed(&pool).await;
    let scratch: PathBuf =
        std::env::temp_dir().join(format!("parlour-backup-flow-{}", Uuid::new_v4()));
    let root = scratch.join("server");
    let backups = scratch.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let export_dir = parlour_server::export::export_dir(&root.join("cache"));

    // ================= 1. Off: nothing queued, nothing written.
    let queued = parlour_server::backup::enqueue_nightly(&pool, None, Utc::now())
        .await
        .unwrap();
    assert_eq!(queued, None, "no backup folder, no nightly job");
    let (nightly_jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs WHERE kind = 'export_bundle'
         AND payload ->> 'trigger' = 'nightly'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nightly_jobs, 0);
    // A nightly job that runs on a server without the setting (it was
    // unset between queueing and running) fails with the reason.
    let stray = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        json!({"trigger": "nightly"}),
    )
    .await
    .unwrap();
    let claimed = parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::ExportBundle)
        .await
        .unwrap()
        .expect("the stray job");
    assert_eq!(claimed.id, stray);
    parlour_server::export::run_export_job_claimed(pool.clone(), claimed, &export_dir, None).await;
    let stray_job = parlour_jobs::get(&pool, stray).await.unwrap().unwrap();
    assert_eq!(stray_job.status, "failed");
    assert!(
        stray_job.errors.to_string().contains("PARLOUR_BACKUP_DIR"),
        "{}",
        stray_job.errors
    );
    assert!(names(&backups).is_empty(), "nothing written while off");
    assert!(!export_dir.exists(), "nor in the export cache");
    yesterday(&pool, stray).await;

    // ================= 2. A night: write, then retention.
    // Older nightly bundles, a crash's leftover, and files that aren't
    // Parlour's nightly bundles at all.
    let old = [
        "parlour-backup-20260101-073000.zip",
        "parlour-backup-20260102-073000.zip",
        "parlour-backup-20260103-073000.zip",
        "parlour-backup-20260104-073000.zip",
    ];
    for name in old {
        std::fs::write(backups.join(name), b"an older night").unwrap();
    }
    std::fs::write(
        backups.join("parlour-backup-20260104-073000.zip.part"),
        b"half",
    )
    .unwrap();
    std::fs::write(backups.join("notes.txt"), b"the admin's own file").unwrap();
    std::fs::write(
        backups.join("parlour-export-20260101-120000.zip"),
        b"a manual download",
    )
    .unwrap();
    // The job that wrote the oldest one, from an earlier night.
    let oldest_job = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        json!({"trigger": "nightly"}),
    )
    .await
    .unwrap();
    parlour_jobs::claim_by_id(&pool, oldest_job).await.unwrap();
    parlour_jobs::set_result(&pool, oldest_job, json!({"file_name": old[0], "bytes": 14}))
        .await
        .unwrap();
    parlour_jobs::complete(&pool, oldest_job).await.unwrap();
    yesterday(&pool, oldest_job).await;

    let settings = BackupSettings {
        dir: backups.clone(),
        keep: 3,
    };
    let server = serve(&pool, &root, Some(settings.clone())).await;
    let admin = server.login("jordan", ADMIN_PASSWORD).await;
    parlour_server::export::spawn_worker(pool.clone(), export_dir.clone(), Some(settings.clone()));

    let night_one = parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
        .await
        .unwrap()
        .expect("tonight's backup is queued");
    assert_eq!(
        parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
            .await
            .unwrap(),
        None,
        "once a day"
    );
    let job = finished(&pool, night_one).await;
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    let result = job.result.clone().unwrap();
    let file_name = result["file_name"].as_str().unwrap().to_string();
    assert!(
        parlour_server::backup::is_backup_name(&file_name),
        "{file_name}"
    );
    assert_eq!(result["trigger"], "nightly");
    assert_eq!(result["folder"], backups.display().to_string());
    assert_eq!(result["keep"], 3);
    assert_eq!(result["hashes_included"], false);
    assert_eq!(result["artwork_included"], false);
    assert_eq!(result["users"], 2);
    assert_eq!(result["profiles"], 3);
    assert_eq!(result["watch_state"], 4);
    let mut removed: Vec<String> = result["removed_by_retention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    removed.sort();
    assert_eq!(
        removed,
        vec![
            old[0].to_string(),
            old[1].to_string(),
            "parlour-backup-20260104-073000.zip.part".to_string(),
        ]
    );
    let mut expected = vec![
        file_name.clone(),
        old[2].to_string(),
        old[3].to_string(),
        "notes.txt".to_string(),
        "parlour-export-20260101-120000.zip".to_string(),
    ];
    expected.sort();
    assert_eq!(names(&backups), expected, "keep 3, strangers untouched");
    assert!(
        !export_dir.exists(),
        "a nightly bundle never lands in the cache"
    );

    // The same bundle a manual export writes: format v1, no hashes.
    let on_disk = std::fs::read(backups.join(&file_name)).unwrap();
    assert_eq!(result["bytes"].as_u64(), Some(on_disk.len() as u64));
    let bundle = open(on_disk.clone());
    assert!(!bundle.manifest.hashes_included);
    assert!(!bundle.manifest.artwork_included);
    assert!(bundle.users.users.iter().all(|u| u.password_hash.is_none()));
    assert_eq!(bundle.users.users.len(), 2);
    assert_eq!(bundle.catalog.items.len(), 2);

    // The old job that wrote a removed bundle stops offering it.
    let oldest = parlour_jobs::get(&pool, oldest_job).await.unwrap().unwrap();
    let oldest_result = oldest.result.unwrap();
    assert_eq!(oldest_result["deleted"], true, "{oldest_result}");
    assert_eq!(oldest_result["removed_by_retention"], true);
    let (status, _) = server
        .req(
            Method::GET,
            &format!("/api/v1/admin/export/{oldest_job}/download"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 404);

    // The Jobs page's download serves it from the backup folder, and
    // Delete is refused: retention owns nightly bundles.
    let res = server
        .send(
            Method::GET,
            &format!("/api/v1/admin/export/{night_one}/download"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(res.status(), 200);
    let disposition = res.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(disposition.contains(&file_name), "{disposition}");
    assert_eq!(res.bytes().await.unwrap().to_vec(), on_disk);
    let (status, body) = server
        .req(
            Method::DELETE,
            &format!("/api/v1/admin/export/{night_one}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(backups.join(&file_name).exists());

    // The Activity feed records it as the system's, not an admin's.
    let feed = activity_summaries(&pool).await;
    assert!(
        feed.contains(&(
            "export.completed".to_string(),
            "Nightly backup written".to_string(),
            None
        )),
        "{feed:?}"
    );

    // ================= 3. A bad folder fails cleanly; the next night retries.
    yesterday(&pool, night_one).await;
    let unmounted = scratch.join("backups-unmounted");
    std::fs::rename(&backups, &unmounted).unwrap();
    let night_two = parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
        .await
        .unwrap()
        .expect("the next night queues again");
    let job = finished(&pool, night_two).await;
    assert_eq!(job.status, "failed");
    let message = job.errors.to_string();
    assert!(
        message.contains(&backups.display().to_string()),
        "{message}"
    );
    assert!(message.contains("doesn't exist"), "{message}");
    assert!(!backups.exists(), "Parlour never creates the folder");
    assert!(!export_dir.exists());
    // Surfaced: the Activity feed, and a notice on the Dashboard.
    let feed = activity_summaries(&pool).await;
    assert!(
        feed.iter()
            .any(|(kind, summary, user)| kind == "export.failed"
                && summary == "Nightly backup failed"
                && user.is_none()),
        "{feed:?}"
    );
    parlour_server::notices::reconcile_with(&pool, &[])
        .await
        .unwrap();
    let (notice,): (String,) = sqlx::query_as("SELECT summary FROM notices WHERE key = $1")
        .bind(format!("job_failed:{night_two}"))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(notice, "Nightly backup failed");
    // No retry storm: the same day queues nothing more.
    assert_eq!(
        parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
            .await
            .unwrap(),
        None
    );

    // The folder comes back but can't be written.
    std::fs::rename(&unmounted, &backups).unwrap();
    let before = names(&backups);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable_anyway = std::fs::write(backups.join("probe"), b"").is_ok();
    if writable_anyway {
        // Running as root: permissions don't stop the write.
        let _ = std::fs::remove_file(backups.join("probe"));
        eprintln!("skipping the unwritable-folder check: this process ignores folder modes");
    } else {
        yesterday(&pool, night_two).await;
        let night_three =
            parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
                .await
                .unwrap()
                .expect("queued");
        let job = finished(&pool, night_three).await;
        assert_eq!(job.status, "failed");
        let message = job.errors.to_string();
        assert!(message.contains("can't be written"), "{message}");
        assert!(
            message.contains(&backups.display().to_string()),
            "{message}"
        );
        assert_eq!(names(&backups), before, "nothing written, nothing deleted");
        yesterday(&pool, night_three).await;
    }
    std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable_anyway {
        yesterday(&pool, night_two).await;
    }

    // The next night, with the folder back, succeeds. (A second apart,
    // so its name differs from the first night's.)
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let night_four = parlour_server::backup::enqueue_nightly(&pool, Some(&settings), Utc::now())
        .await
        .unwrap()
        .expect("queued");
    let job = finished(&pool, night_four).await;
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    let latest = job.result.unwrap()["file_name"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(latest, file_name);
    let left: Vec<String> = names(&backups)
        .into_iter()
        .filter(|n| parlour_server::backup::is_backup_name(n))
        .collect();
    let mut want = vec![latest.clone(), file_name.clone(), old[3].to_string()];
    want.sort();
    assert_eq!(left, want, "still the newest three");

    // ================= 4. Restore = XFER3, onto a fresh install.
    let admin_pool = pool.clone();
    sqlx::query("DROP DATABASE IF EXISTS parlour_backup_flow_target WITH (FORCE)")
        .execute(&admin_pool)
        .await
        .unwrap();
    sqlx::query("CREATE DATABASE parlour_backup_flow_target")
        .execute(&admin_pool)
        .await
        .expect("create a target database (the test role needs CREATEDB)");
    let target_url = with_database(&url, TARGET);
    let target = parlour_db::connect(&target_url).await.expect("connect");
    parlour_db::migrate(&target).await.expect("migrate target");
    parlour_server::auth::bootstrap_admin(&target, &test_config(&target_url))
        .await
        .expect("bootstrap target");
    let new = serve(&target, &scratch.join("target"), None).await;
    let new_admin = new.login("jordan", ADMIN_PASSWORD).await;
    let nightly_bundle = std::fs::read(backups.join(&latest)).unwrap();
    let (status, uploaded) = new.upload(&new_admin, nightly_bundle).await;
    assert_eq!(status, 201, "{uploaded}");
    let report = &uploaded["report"];
    assert_eq!(report["manifest"]["hashes_included"], false);
    assert_eq!(report["conflicts_total"], 0, "{report}");
    let id = uploaded["id"].as_str().unwrap();
    let (status, job) = new
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{id}/apply"),
            Some(&new_admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 202, "{job}");
    let job = finished(
        &target,
        Uuid::parse_str(job["id"].as_str().unwrap()).unwrap(),
    )
    .await;
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    let applied = job.result.unwrap();
    let users: Vec<(String, String, String)> = applied["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["username"].as_str().unwrap().to_string(),
                u["action"].as_str().unwrap().to_string(),
                u["sign_in"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        users.contains(&("jordan".into(), "adopt".into(), "kept".into())),
        "D38: {users:?}"
    );
    assert!(
        users.contains(&("ada".into(), "create".into(), "locked".into())),
        "D37: {users:?}"
    );

    // Same users, profiles and watch history.
    let usernames = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT username FROM users ORDER BY username")
            .fetch_all(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        usernames(target.clone()).await,
        usernames(pool.clone()).await
    );
    assert_eq!(profiles(&target).await, profiles(&pool).await);
    let history = watch(&pool).await;
    assert_eq!(history.len(), 4);
    assert_eq!(watch(&target).await, history);
    // No hashes in a nightly bundle: ada is locked until an admin sets
    // a password (D37); the adopted admin keeps this install's.
    assert!(new.try_login("ada", ADA_PASSWORD).await.is_none());
    assert!(new.try_login("jordan", ADMIN_PASSWORD).await.is_some());

    target.close().await;
    sqlx::query("DROP DATABASE IF EXISTS parlour_backup_flow_target WITH (FORCE)")
        .execute(&admin_pool)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}
