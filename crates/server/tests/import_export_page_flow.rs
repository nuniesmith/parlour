//! Server → Import/Export (XFER8) against real Postgres, through the
//! endpoints the page calls. It is also the Phase 8 acceptance: the
//! whole server state survives a host move.
//!
//! 1. **Admin only.** Every endpoint the page uses answers 404 to a
//!    signed-in non-admin and 401 to nobody.
//! 2. **Host move.** The source server (this test's database) is
//!    exported from the page with hashes; the bundle is listed among
//!    the page's export jobs and downloads. A fresh install (a database
//!    the test creates beside the source) uploads it, finds it among
//!    the waiting uploads, and reads the dry run, which already shows
//!    D38's take-over and every count, and has changed nothing. A
//!    bundle without hashes shows D37's locked accounts before apply.
//!    Applying with a replacement folder reports exactly what the dry
//!    run said, deletes the upload, and leaves the target exporting the
//!    same users, profiles and watch history as the source. The old
//!    passwords work.
//! 3. **Creates, updates, conflicts.** A later bundle from the source
//!    (a new user, a newer position) imported onto the target, which
//!    meanwhile has its own account under that new user's name: the
//!    dry run shows the create, the update and the conflict, and the
//!    apply does just that.
//! 4. **Sidecar status** follows XFER5's opt-in, the folder check and
//!    the notice, and counts the ledgers XFER4–XFER6 keep.
//! 5. **Backup settings (D39)** round-trip; an unset folder (saved off,
//!    or nothing saved and no environment default) queues no nightly
//!    backup; a saved folder overrides the environment; hashes go into
//!    a nightly bundle only when the admin turned them on.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a fresh
//! throwaway database (its role must be allowed to create databases)
//! and run `cargo test -- --ignored`. The target database is created
//! and dropped by the test. Every password and PIN here is a throwaway
//! made up for this test.

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

const TARGET: &str = "parlour_xfer8_flow_target";

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

/// A fresh, migrated install beside the source database, with its
/// first admin bootstrapped (same name as the source's, as a fresh
/// install of the same household would have).
async fn fresh_target(admin: &sqlx::PgPool, source_url: &str) -> sqlx::PgPool {
    sqlx::query("DROP DATABASE IF EXISTS parlour_xfer8_flow_target WITH (FORCE)")
        .execute(admin)
        .await
        .unwrap();
    sqlx::query("CREATE DATABASE parlour_xfer8_flow_target")
        .execute(admin)
        .await
        .expect("create the target database (the test role needs CREATEDB)");
    let url = with_database(source_url, TARGET);
    let pool = parlour_db::connect(&url).await.expect("connect target");
    parlour_db::migrate(&pool).await.expect("migrate target");
    parlour_server::auth::bootstrap_admin(&pool, &test_config(&url))
        .await
        .expect("bootstrap target");
    pool
}

struct Server {
    http: reqwest::Client,
    base: String,
}

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

    async fn wait(&self, cookie: &str, id: &str) -> Value {
        for _ in 0..300 {
            let (_, job) = self
                .req(
                    Method::GET,
                    &format!("/api/v1/admin/jobs/{id}"),
                    Some(cookie),
                    None,
                )
                .await;
            match job["status"].as_str() {
                Some("succeeded") | Some("failed") => return job,
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        panic!("job {id} did not finish");
    }

    /// The page's Export button, then its Download link.
    async fn export(&self, cookie: &str, include_hashes: bool) -> (String, Vec<u8>) {
        let (status, job) = self
            .req(
                Method::POST,
                "/api/v1/admin/export",
                Some(cookie),
                Some(json!({"include_hashes": include_hashes, "include_artwork": false})),
            )
            .await;
        assert_eq!(status, 202, "{job}");
        let id = job["id"].as_str().unwrap().to_string();
        let job = self.wait(cookie, &id).await;
        assert_eq!(job["status"], "succeeded", "{job}");
        // The page lists it among its own jobs.
        let (status, listed) = self
            .req(
                Method::GET,
                "/api/v1/admin/jobs?kind=export_bundle&limit=40",
                Some(cookie),
                None,
            )
            .await;
        assert_eq!(status, 200);
        let row = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id.as_str())
            .expect("the export is on the page's list")
            .clone();
        assert_eq!(row["result"]["hashes_included"], include_hashes);
        let res = self
            .send(
                Method::GET,
                &format!("/api/v1/admin/export/{id}/download"),
                Some(cookie),
                None,
            )
            .await;
        assert_eq!(res.status(), 200);
        (id, res.bytes().await.unwrap().to_vec())
    }

    /// The page's Upload and check.
    async fn upload(&self, cookie: &str, bytes: Vec<u8>) -> Value {
        let res = self
            .http
            .post(format!("{}/api/v1/admin/import", self.base))
            .header("Content-Type", "application/zip")
            .header("Cookie", format!("parlour_session={cookie}"))
            .body(bytes)
            .send()
            .await
            .expect("upload");
        assert_eq!(res.status(), 201);
        res.json().await.unwrap()
    }

    async fn pending(&self, cookie: &str) -> Vec<String> {
        let (status, list) = self
            .req(Method::GET, "/api/v1/admin/import", Some(cookie), None)
            .await;
        assert_eq!(status, 200, "{list}");
        list.as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect()
    }

    async fn sidecars(&self, cookie: &str, library: &str) -> Value {
        let (status, list) = self
            .req(Method::GET, "/api/v1/admin/sidecars", Some(cookie), None)
            .await;
        assert_eq!(status, 200, "{list}");
        list.as_array()
            .unwrap()
            .iter()
            .find(|l| l["name"] == library)
            .unwrap_or_else(|| panic!("{library} in {list}"))
            .clone()
    }
}

fn open(bytes: Vec<u8>) -> Bundle {
    BundleReader::open(Cursor::new(bytes))
        .expect("a bundle the reader accepts")
        .1
}

fn user<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == name)
        .unwrap_or_else(|| panic!("{name} in {report}"))
}

/// Ids the source seed hands back.
struct Seeded {
    films: Uuid,
    shows: Uuid,
    movie: Uuid,
    ada_profile: Uuid,
}

/// The source: a second user with a PIN'd profile, three libraries
/// (one of them audiobooks, one with a writable folder), a movie and
/// a series down to an episode, watch state and recent searches.
async fn seed(pool: &sqlx::PgPool, shows_dir: &Path) -> Seeded {
    let ada = parlour_server::auth::create_user(pool, "ada", ADA_PASSWORD, false)
        .await
        .expect("ada");
    let (ada_profile,): (Uuid,) = sqlx::query_as("SELECT id FROM profiles WHERE user_id = $1")
        .bind(ada)
        .fetch_one(pool)
        .await
        .unwrap();
    let pin = parlour_server::auth::hash_password("4321").unwrap();
    let (kids,): (Uuid,) = sqlx::query_as(
        "INSERT INTO profiles (user_id, name, pin_hash) VALUES ($1, 'Kids', $2) RETURNING id",
    )
    .bind(ada)
    .bind(&pin)
    .fetch_one(pool)
    .await
    .unwrap();
    let library = |name: &'static str, kind: &'static str, path: String| async move {
        let (id,): (Uuid,) =
            sqlx::query_as("INSERT INTO libraries (name, kind) VALUES ($1, $2) RETURNING id")
                .bind(name)
                .bind(kind)
                .fetch_one(pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, $2)")
            .bind(id)
            .bind(path)
            .execute(pool)
            .await
            .unwrap();
        id
    };
    let films = library("Films", "movies", "/old-host/media/films".to_string()).await;
    let shows = library("Shows", "shows", shows_dir.display().to_string()).await;
    library("Books", "audiobooks", "/old-host/media/books".to_string()).await;
    for (lib, profile) in [(films, ada_profile), (films, kids), (shows, ada_profile)] {
        sqlx::query("INSERT INTO library_access (library_id, profile_id) VALUES ($1, $2)")
            .bind(lib)
            .bind(profile)
            .execute(pool)
            .await
            .unwrap();
    }
    let item = |lib: Uuid, parent: Option<Uuid>, kind: &'static str, key: &'static str| async move {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_items (library_id, parent_id, kind, stable_key, title, sort_title,
                                      metadata_status, last_metadata_at)
             VALUES ($1, $2, $3, $4, $4, $4, 'matched', now()) RETURNING id",
        )
        .bind(lib)
        .bind(parent)
        .bind(kind)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap();
        id
    };
    let movie = item(films, None, "movie", "movie:lanterns:lanterns:2021").await;
    let show = item(shows, None, "series", "series:harbor:2019").await;
    let season = item(shows, Some(show), "season", "series:harbor:2019:s01").await;
    let episode = item(
        shows,
        Some(season),
        "episode",
        "series:harbor:2019:s01:e001",
    )
    .await;
    sqlx::query(
        "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played,
                                is_favorite, play_count, last_played_at)
         VALUES ($1, $2, 120000, 5400000, false, true, 1, now()),
                ($1, $3, 0, 1500000, true, false, 2, now()),
                ($4, $2, 600000, 5400000, false, false, 1, now())",
    )
    .bind(ada_profile)
    .bind(movie)
    .bind(episode)
    .bind(kids)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO recent_searches (profile_id, query, searched_at)
         VALUES ($1, 'harbor', now() - interval '1 hour'), ($1, 'lanterns', now())",
    )
    .bind(ada_profile)
    .execute(pool)
    .await
    .unwrap();
    // What the sidecar machinery records (XFER4–XFER6), as its jobs
    // would leave it: the movie's sidecar and NFO read, an episode's
    // sidecar overridden by a fix-match, and the files Parlour wrote in
    // the show's folder.
    sqlx::query(
        "INSERT INTO item_sidecars (item_id, fingerprint, document, overridden_at)
         VALUES ($1, 'fp-movie', '{}', NULL), ($2, 'fp-episode', '{}', now()),
                ($3, 'fp-show', '{}', NULL)",
    )
    .bind(movie)
    .bind(episode)
    .bind(show)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO item_nfos (item_id, fingerprint, document) VALUES ($1, 'fp', '{}')")
        .bind(movie)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sidecar_nfos (path, item_id, sha256) VALUES ($1, $2, 'aa')")
        .bind(
            shows_dir
                .join("Harbor (2019)/tvshow.nfo")
                .display()
                .to_string(),
        )
        .bind(show)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sidecar_artwork (path, item_id, kind, sha256)
         VALUES ($1, $3, 'poster', 'bb'), ($2, $3, 'backdrop', 'cc')",
    )
    .bind(
        shows_dir
            .join("Harbor (2019)/poster.jpg")
            .display()
            .to_string(),
    )
    .bind(
        shows_dir
            .join("Harbor (2019)/backdrop.jpg")
            .display()
            .to_string(),
    )
    .bind(show)
    .execute(pool)
    .await
    .unwrap();
    Seeded {
        films,
        shows,
        movie,
        ada_profile,
    }
}

async fn counts(pool: &sqlx::PgPool) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT unnest(ARRAY[
             (SELECT count(*) FROM users), (SELECT count(*) FROM profiles),
             (SELECT count(*) FROM libraries), (SELECT count(*) FROM media_items),
             (SELECT count(*) FROM user_data), (SELECT count(*) FROM recent_searches)])",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Claim a queued nightly job and run it as the export worker would,
/// with `env` as the environment's default.
async fn run_nightly(
    pool: &sqlx::PgPool,
    id: Uuid,
    export_dir: &Path,
    env: Option<&BackupSettings>,
) {
    assert!(parlour_jobs::claim_by_id(pool, id).await.unwrap());
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    parlour_server::export::run_export_job_claimed(pool.clone(), job, export_dir, env).await;
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL (throwaway Postgres)"]
async fn import_export_page_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &test_config(&url))
        .await
        .expect("bootstrap");
    let scratch: PathBuf =
        std::env::temp_dir().join(format!("parlour-xfer8-flow-{}", Uuid::new_v4()));
    let shows_dir = scratch.join("media/shows");
    let backups = scratch.join("backups");
    let env_backups = scratch.join("env-backups");
    for dir in [&shows_dir, &backups, &env_backups] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let seeded = seed(&pool, &shows_dir).await;
    let source = serve(&pool, &scratch.join("source"), None).await;
    let admin = source.login("jordan", ADMIN_PASSWORD).await;

    // ================= 1. Admin only.
    let ada = source.login("ada", ADA_PASSWORD).await;
    let some_id = Uuid::new_v4();
    let page_routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/v1/admin/backup".into(), None),
        (
            Method::PUT,
            "/api/v1/admin/backup".into(),
            Some(json!({"folder": null, "keep": 7})),
        ),
        (Method::DELETE, "/api/v1/admin/backup".into(), None),
        (Method::GET, "/api/v1/admin/sidecars".into(), None),
        (Method::GET, "/api/v1/admin/import".into(), None),
        (
            Method::GET,
            "/api/v1/admin/jobs?kind=export_bundle".into(),
            None,
        ),
        (Method::GET, "/api/v1/admin/export/estimate".into(), None),
        (Method::POST, "/api/v1/admin/export".into(), Some(json!({}))),
        (
            Method::POST,
            format!("/api/v1/admin/import/{some_id}/dry-run"),
            Some(json!({})),
        ),
        (
            Method::POST,
            format!("/api/v1/admin/import/{some_id}/apply"),
            Some(json!({})),
        ),
        (
            Method::DELETE,
            format!("/api/v1/admin/import/{some_id}"),
            None,
        ),
    ];
    for (method, path, body) in &page_routes {
        let (status, _) = source
            .req(method.clone(), path, Some(&ada), body.clone())
            .await;
        assert_eq!(status, 404, "{method} {path} as a non-admin");
        let (status, _) = source.req(method.clone(), path, None, body.clone()).await;
        assert_eq!(status, 401, "{method} {path} signed out");
    }
    let (status, _) = source
        .req(
            Method::GET,
            "/api/v1/admin/jobs?kind=not_a_kind",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 400, "an unknown job kind is refused");
    // Nothing changed on the way: no backup setting was saved.
    let (saved,): (i64,) = sqlx::query_as("SELECT count(*) FROM backup_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(saved, 0);

    // ================= 2. Host move.
    let (export_id, with_hashes) = source.export(&admin, true).await;
    let (_, without_hashes) = source.export(&admin, false).await;
    // Only export jobs on the export list.
    let (_, listed) = source
        .req(
            Method::GET,
            "/api/v1/admin/jobs?kind=export_bundle&limit=40",
            Some(&admin),
            None,
        )
        .await;
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .all(|j| j["kind"] == "export_bundle"));
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|j| j["id"] == export_id.as_str()));
    let source_bundle = open(with_hashes.clone());
    assert!(source_bundle.manifest.hashes_included);

    let admin_pool = pool.clone();
    let target_pool = fresh_target(&admin_pool, &url).await;
    let target = serve(&target_pool, &scratch.join("target"), None).await;
    let t_admin = target.login("jordan", ADMIN_PASSWORD).await;
    assert!(target.pending(&t_admin).await.is_empty());
    let fresh_counts = counts(&target_pool).await;

    // A bundle without hashes: D37's lock is visible before apply.
    let up = target.upload(&t_admin, without_hashes).await;
    let report = &up["report"];
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["manifest"]["hashes_included"], false);
    assert_eq!(user(report, "ada")["action"], "create");
    assert_eq!(user(report, "ada")["sign_in"], "locked");
    assert_eq!(user(report, "jordan")["action"], "adopt");
    assert_eq!(user(report, "jordan")["sign_in"], "kept");
    assert!(
        report["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("no password hashes")),
        "{report}"
    );
    // Discarded from the page: gone from the list.
    let no_hash_id = up["id"].as_str().unwrap().to_string();
    assert_eq!(target.pending(&t_admin).await, vec![no_hash_id.clone()]);
    let (status, _) = target
        .req(
            Method::DELETE,
            &format!("/api/v1/admin/import/{no_hash_id}"),
            Some(&t_admin),
            None,
        )
        .await;
    assert_eq!(status, 204);
    assert!(target.pending(&t_admin).await.is_empty());

    // The bundle with hashes: D38's take-over, before apply.
    let up = target.upload(&t_admin, with_hashes).await;
    let upload_id = up["id"].as_str().unwrap().to_string();
    assert_eq!(target.pending(&t_admin).await, vec![upload_id.clone()]);
    let first = &up["report"];
    assert_eq!(user(first, "jordan")["action"], "adopt");
    assert_eq!(user(first, "jordan")["sign_in"], "from_bundle");
    assert_eq!(user(first, "ada")["action"], "create");
    assert_eq!(user(first, "ada")["sign_in"], "from_bundle");
    assert_eq!(first["conflicts_total"], 0);
    // The page's "Check again with these folders": Shows moves.
    let new_shows = scratch.join("new-host/shows");
    std::fs::create_dir_all(&new_shows).unwrap();
    let options = json!({"library_paths": {"Shows": [new_shows.display().to_string()]}});
    let (status, dry) = target
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{upload_id}/dry-run"),
            Some(&t_admin),
            Some(options.clone()),
        )
        .await;
    assert_eq!(status, 200, "{dry}");
    let shows_row = dry["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == "Shows")
        .unwrap();
    assert_eq!(shows_row["action"], "create");
    assert_eq!(
        shows_row["paths_added"],
        json!([{"path": new_shows.display().to_string(), "exists_here": true}])
    );
    assert_eq!(
        shows_row["bundle_paths"][0]["exists_here"], true,
        "same scratch host"
    );
    assert_eq!(dry["items"]["created"], 4);
    assert_eq!(dry["watch_state"]["created"], 3);
    assert_eq!(dry["recent_searches"]["created"], 2);
    assert_eq!(
        dry["profiles"]["created"], 2,
        "ada's two; jordan's is adopted"
    );
    assert_eq!(
        counts(&target_pool).await,
        fresh_counts,
        "the dry run changed nothing"
    );
    // Apply from the page: the job's report is the dry run's.
    let (status, job) = target
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{upload_id}/apply"),
            Some(&t_admin),
            Some(options),
        )
        .await;
    assert_eq!(status, 202, "{job}");
    let job = target.wait(&t_admin, job["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let mut applied = job["result"].clone();
    applied["dry_run"] = json!(true);
    assert_eq!(applied, dry, "the apply did what the dry run said");
    assert!(
        target.pending(&t_admin).await.is_empty(),
        "an applied upload is deleted (it held hashes)"
    );
    let (_, imports) = target
        .req(
            Method::GET,
            "/api/v1/admin/jobs?kind=import_bundle",
            Some(&t_admin),
            None,
        )
        .await;
    assert_eq!(imports.as_array().unwrap().len(), 1);

    // Same users, profiles and watch history; the old passwords work.
    let t_ada = target.login("ada", ADA_PASSWORD).await;
    assert!(!t_ada.is_empty());
    let t_admin = target.login("jordan", ADMIN_PASSWORD).await;
    let (_, moved) = target.export(&t_admin, true).await;
    let moved = open(moved);
    assert_eq!(moved.users, source_bundle.users);
    assert_eq!(moved.profiles, source_bundle.profiles);
    assert_eq!(moved.user_data.profiles, source_bundle.user_data.profiles);
    for entry in &source_bundle.user_data.activity {
        assert!(moved.user_data.activity.contains(entry), "{entry:?}");
    }
    let names = |b: &Bundle| -> Vec<(String, String)> {
        b.libraries
            .libraries
            .iter()
            .map(|l| (l.name.clone(), format!("{:?}", l.kind)))
            .collect()
    };
    assert_eq!(names(&moved), names(&source_bundle));
    assert_eq!(moved.catalog.items.len(), source_bundle.catalog.items.len());
    let moved_shows = moved
        .libraries
        .libraries
        .iter()
        .find(|l| l.name == "Shows")
        .unwrap();
    assert_eq!(moved_shows.paths, vec![new_shows.display().to_string()]);

    // ================= 3. Creates, updates, conflicts.
    parlour_server::auth::create_user(&pool, "zed", ADMIN_PASSWORD, false)
        .await
        .unwrap();
    parlour_server::auth::create_user(&target_pool, "zed", ADMIN_PASSWORD, false)
        .await
        .unwrap();
    let mira = "mira";
    parlour_server::auth::create_user(&pool, mira, ADMIN_PASSWORD, false)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE user_data SET position_ms = 900000, updated_at = now() + interval '1 minute'
         WHERE profile_id = $1 AND item_id = $2",
    )
    .bind(seeded.ada_profile)
    .bind(seeded.movie)
    .execute(&pool)
    .await
    .unwrap();
    let (_, later) = source.export(&admin, true).await;
    let up = target.upload(&t_admin, later).await;
    let report = &up["report"];
    assert_eq!(user(report, mira)["action"], "create");
    assert_eq!(user(report, "zed")["action"], "conflict");
    assert_eq!(user(report, "zed")["sign_in"], "none");
    assert_eq!(user(report, "ada")["action"], "existing");
    assert_eq!(user(report, "jordan")["action"], "existing");
    assert_eq!(report["watch_state"]["updated"], 1, "{report}");
    assert!(report["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["what"] == "user" && c["key"] == "zed"));
    let later_id = up["id"].as_str().unwrap().to_string();
    let (status, job) = target
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{later_id}/apply"),
            Some(&t_admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 202);
    let job = target.wait(&t_admin, job["id"].as_str().unwrap()).await;
    assert_eq!(job["status"], "succeeded", "{job}");
    let (position,): (i64,) = sqlx::query_as(
        "SELECT d.position_ms FROM user_data d
         JOIN profiles p ON p.id = d.profile_id JOIN users u ON u.id = p.user_id
         JOIN media_items i ON i.id = d.item_id
         WHERE u.username = 'ada' AND p.name = 'ada' AND i.stable_key = 'movie:lanterns:lanterns:2021'",
    )
    .fetch_one(&target_pool)
    .await
    .unwrap();
    assert_eq!(position, 900000, "the newer position landed");
    let (zeds,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM users WHERE lower(username) = 'zed'")
            .fetch_one(&target_pool)
            .await
            .unwrap();
    assert_eq!(zeds, 1, "the conflict was skipped, not merged");
    assert!(target.try_login(mira, ADMIN_PASSWORD).await.is_some());

    // ================= 4. Sidecar status.
    let films = source.sidecars(&admin, "Films").await;
    assert_eq!(films["supported"], true);
    assert_eq!(films["write_sidecars"], false);
    assert_eq!(films["folders"][0]["writable"], false);
    assert!(films["folders"][0]["problem"].is_string());
    assert_eq!(films["items"], 1);
    assert_eq!(films["sidecars"], 1);
    assert_eq!(films["nfos_read"], 1);
    assert_eq!(films["nfos_written"], 0);
    let shows = source.sidecars(&admin, "Shows").await;
    assert_eq!(shows["folders"][0]["writable"], true);
    assert_eq!(shows["items"], 2, "the series and its episode");
    assert_eq!(shows["sidecars"], 2);
    assert_eq!(shows["sidecars_overridden"], 1);
    assert_eq!(shows["nfos_written"], 1);
    assert_eq!(shows["artwork_written"], 2);
    let books = source.sidecars(&admin, "Books").await;
    assert_eq!(
        books["supported"], true,
        "audiobooks get sidecars since AB6"
    );
    assert_eq!(books["write_sidecars"], false);
    // XFER5's switch, seen from this page.
    for (lib, on) in [(seeded.shows, true), (seeded.films, true)] {
        let (status, _) = source
            .req(
                Method::PUT,
                &format!("/api/v1/admin/libraries/{lib}/write-sidecars"),
                Some(&admin),
                Some(json!({"enabled": on})),
            )
            .await;
        assert_eq!(status, 200);
    }
    parlour_server::notices::reconcile_with(&pool, &[])
        .await
        .unwrap();
    let shows = source.sidecars(&admin, "Shows").await;
    assert_eq!(shows["write_sidecars"], true);
    assert_eq!(shows["unwritable_notice"], false);
    let films = source.sidecars(&admin, "Films").await;
    assert_eq!(films["write_sidecars"], true);
    assert_eq!(
        films["unwritable_notice"], true,
        "on, but the folder can't be written: XFER5's notice is up"
    );
    let (status, _) = source
        .req(
            Method::PUT,
            &format!("/api/v1/admin/libraries/{}/write-sidecars", seeded.films),
            Some(&admin),
            Some(json!({"enabled": false})),
        )
        .await;
    assert_eq!(status, 200);
    parlour_server::notices::reconcile_with(&pool, &[])
        .await
        .unwrap();
    let films = source.sidecars(&admin, "Films").await;
    assert_eq!(films["write_sidecars"], false);
    assert_eq!(films["unwritable_notice"], false);

    // ================= 5. Backup settings (D39).
    let export_dir = parlour_server::export::export_dir(&scratch.join("source/cache"));
    let env = BackupSettings {
        dir: env_backups.clone(),
        keep: 4,
        include_hashes: false,
    };
    let nightly_jobs = || async {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM jobs WHERE kind = 'export_bundle'
             AND payload ->> 'trigger' = 'nightly'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        n
    };
    // Nothing saved, no environment default: off, nothing queued.
    let (status, view) = source
        .req(Method::GET, "/api/v1/admin/backup", Some(&admin), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(view["source"], "environment");
    assert!(view["folder"].is_null(), "{view}");
    assert_eq!(view["keep"], 7);
    assert_eq!(view["include_hashes"], false);
    let day = chrono::Duration::days(1);
    let now = Utc::now();
    assert_eq!(
        parlour_server::backup::enqueue_nightly(&pool, None, now)
            .await
            .unwrap(),
        None
    );
    // A server whose environment sets a folder shows it as the default.
    let with_env = serve(&pool, &scratch.join("source-env"), Some(env.clone())).await;
    let (_, view) = with_env
        .req(Method::GET, "/api/v1/admin/backup", Some(&admin), None)
        .await;
    assert_eq!(view["source"], "environment");
    assert_eq!(view["folder"], env_backups.display().to_string());
    assert_eq!(view["keep"], 4);
    assert!(view["folder_problem"].is_null(), "{view}");
    // The page refuses what PARLOUR_BACKUP_DIR would (400), and a body
    // it doesn't know (the strict JSON body's 422).
    for (bad, want) in [
        (json!({"folder": "backups", "keep": 7}), 400),
        (json!({"folder": "/backups", "keep": 0}), 400),
        (json!({"folder": "/backups", "keep": 366}), 400),
        (
            json!({"folder": "/backups", "keep": 7, "schedule": "hourly"}),
            422,
        ),
    ] {
        let (status, _) = source
            .req(
                Method::PUT,
                "/api/v1/admin/backup",
                Some(&admin),
                Some(bad.clone()),
            )
            .await;
        assert_eq!(status, want, "{bad}");
    }
    // Saved off: overrides the environment's folder.
    let (status, view) = with_env
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": "  ", "keep": 5})),
        )
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["source"], "saved");
    assert!(view["folder"].is_null());
    assert_eq!(view["keep"], 5);
    assert_eq!(view["updated_by"], "jordan");
    assert_eq!(
        parlour_server::backup::enqueue_nightly(&pool, Some(&env), now)
            .await
            .unwrap(),
        None,
        "saved off beats the environment"
    );
    assert_eq!(nightly_jobs().await, 0);
    // Saved on: a folder that doesn't exist is saved, and the page
    // says tonight's backup would fail.
    let missing = scratch.join("not-mounted");
    let (_, view) = source
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": missing.display().to_string(), "keep": 3})),
        )
        .await;
    assert!(
        view["folder_problem"]
            .as_str()
            .unwrap()
            .contains("doesn't exist"),
        "{view}"
    );
    assert!(!missing.exists(), "never created");
    // The real folder: round-trips, and the dashboard follows.
    let (status, view) = source
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": backups.display().to_string(), "keep": 3,
                        "include_hashes": false})),
        )
        .await;
    assert_eq!(status, 200);
    let (_, again) = source
        .req(Method::GET, "/api/v1/admin/backup", Some(&admin), None)
        .await;
    assert_eq!(again, view, "round trip");
    assert_eq!(again["folder"], backups.display().to_string());
    assert_eq!(again["keep"], 3);
    assert!(again["folder_problem"].is_null());
    let (_, info) = source
        .req(Method::GET, "/api/v1/admin/server", Some(&admin), None)
        .await;
    assert_eq!(info["backups"]["folder"], backups.display().to_string());
    assert_eq!(info["backups"]["source"], "saved");
    // Tonight: the saved folder, not the environment's; no hashes.
    let night = parlour_server::backup::enqueue_nightly(&pool, Some(&env), now)
        .await
        .unwrap()
        .expect("queued from the saved setting");
    run_nightly(&pool, night, &export_dir, Some(&env)).await;
    let job = parlour_jobs::get(&pool, night).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    let result = job.result.unwrap();
    assert_eq!(result["folder"], backups.display().to_string());
    assert_eq!(result["keep"], 3);
    assert_eq!(result["hashes_included"], false);
    let file = backups.join(result["file_name"].as_str().unwrap());
    assert!(!open(std::fs::read(&file).unwrap()).manifest.hashes_included);
    assert_eq!(
        std::fs::read_dir(&env_backups).unwrap().count(),
        0,
        "nothing in the environment's folder"
    );
    // Hashes only once the admin turns them on.
    let (_, view) = source
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": backups.display().to_string(), "keep": 3,
                        "include_hashes": true})),
        )
        .await;
    assert_eq!(view["include_hashes"], true);
    let night_two = parlour_server::backup::enqueue_nightly(&pool, Some(&env), now + day)
        .await
        .unwrap()
        .expect("the next night");
    let payload = parlour_jobs::get(&pool, night_two)
        .await
        .unwrap()
        .unwrap()
        .payload;
    assert_eq!(payload["include_hashes"], true);
    run_nightly(&pool, night_two, &export_dir, Some(&env)).await;
    let job = parlour_jobs::get(&pool, night_two).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    let result = job.result.unwrap();
    assert_eq!(result["hashes_included"], true);
    let bundle = open(std::fs::read(backups.join(result["file_name"].as_str().unwrap())).unwrap());
    assert!(bundle.manifest.hashes_included);
    assert!(bundle.users.users.iter().all(|u| u.password_hash.is_some()));
    // A job queued with hashes doesn't get them if they were turned off
    // before it ran.
    let night_three = parlour_server::backup::enqueue_nightly(&pool, None, now + day * 2)
        .await
        .unwrap()
        .expect("the night after");
    let (status, _) = source
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": backups.display().to_string(), "keep": 3})),
        )
        .await;
    assert_eq!(status, 200);
    run_nightly(&pool, night_three, &export_dir, None).await;
    let job = parlour_jobs::get(&pool, night_three)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.status, "succeeded", "{:?}", job.errors);
    assert_eq!(job.result.unwrap()["hashes_included"], false);
    // Turned off from the page: the next night queues nothing, and the
    // earlier nights still download from where they were written.
    let (_, view) = source
        .req(
            Method::PUT,
            "/api/v1/admin/backup",
            Some(&admin),
            Some(json!({"folder": null, "keep": 3})),
        )
        .await;
    assert!(view["folder"].is_null());
    let before = nightly_jobs().await;
    assert_eq!(
        parlour_server::backup::enqueue_nightly(&pool, Some(&env), now + day * 3)
            .await
            .unwrap(),
        None,
        "an unset folder queues no nightly backup"
    );
    assert_eq!(nightly_jobs().await, before);
    let res = source
        .send(
            Method::GET,
            &format!("/api/v1/admin/export/{night}/download"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(res.status(), 200);
    // Reset: the environment's default applies again.
    let (status, view) = with_env
        .req(Method::DELETE, "/api/v1/admin/backup", Some(&admin), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(view["source"], "environment");
    assert_eq!(view["folder"], env_backups.display().to_string());
    assert!(
        parlour_server::backup::enqueue_nightly(&pool, Some(&env), now + day * 4)
            .await
            .unwrap()
            .is_some(),
        "the environment's default is back"
    );
    // Every change is in the Activity feed under Exports, by the admin.
    let changes: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT summary, user_id FROM activity WHERE kind = 'export.backup_settings'
         ORDER BY created_at, id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    // Off, a missing folder, the real one, hashes on, hashes off, off,
    // reset: seven saved changes (refused ones aren't changes).
    assert_eq!(changes.len(), 7, "{changes:?}");
    assert!(changes
        .iter()
        .any(|(s, _)| s.contains("with password and PIN hashes")));
    assert!(changes
        .iter()
        .any(|(s, _)| s == "Nightly backup settings reset to the server's environment"));
    let (jordan,): (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE username = 'jordan'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(changes.iter().all(|(_, u)| *u == Some(jordan)));

    // Clean up.
    drop(target_pool);
    sqlx::query("DROP DATABASE IF EXISTS parlour_xfer8_flow_target WITH (FORCE)")
        .execute(&admin_pool)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&scratch);
}
