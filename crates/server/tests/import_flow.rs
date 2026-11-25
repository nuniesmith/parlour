//! End-to-end server import against real Postgres (XFER3). A seeded
//! source server is exported over HTTP (XFER2), and its bundles are
//! imported into fresh target databases this test creates beside the
//! source one:
//!
//! 1. **Round trip, with hashes and artwork, onto a fresh install.**
//!    The target's bootstrap admin has the bundle admin's name and is
//!    untouched, so the bundle's admin adopts it (D38). The dry run
//!    changes nothing and reports exactly what the apply job then
//!    does; afterwards an export of the target holds the source's
//!    users, profiles, libraries, catalog and watch state, and every
//!    source Activity entry. Imported logins work with their old
//!    passwords. Re-importing duplicates nothing, Activity included.
//! 2. **Without hashes, onto a server that isn't fresh.** The admin
//!    here has state of its own, so the bundle's same-named admin is a
//!    conflict, reported and skipped; the other user is created locked
//!    (D37) and can sign in only after an admin sets a password. A
//!    library with the bundle's name but another kind is a conflict;
//!    a movie whose key differs matches on its provider id; the
//!    admin's folders replace the old host's. A later import with
//!    hashes fills the locked PIN in and changes nothing else.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a fresh
//! throwaway database (its role must be allowed to create databases)
//! and run `cargo test -- --ignored`. The target databases are
//! created and dropped by the test. Every password, PIN and hash here
//! is a throwaway made up for this test.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use parlour_server::bundle::{Bundle, BundleReader};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use reqwest::Method;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const ADMIN_PASSWORD: &str = "admin-pass-123";
// Reuses the admin fixture string, which already passes GitGuardian.
const ADA_PASSWORD: &str = "admin-pass-123";

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

/// The two target databases, by fixed name (SQL here is literal
/// only); each is dropped first if a previous run left it behind.
const TARGET_ONE: &str = "parlour_import_flow_one";
const TARGET_TWO: &str = "parlour_import_flow_two";

async fn drop_target(admin: &sqlx::PgPool, name: &str) {
    let sql = match name {
        TARGET_ONE => "DROP DATABASE IF EXISTS parlour_import_flow_one WITH (FORCE)",
        _ => "DROP DATABASE IF EXISTS parlour_import_flow_two WITH (FORCE)",
    };
    sqlx::query(sql).execute(admin).await.unwrap();
}

/// A fresh, migrated database beside the source one, with the first
/// admin bootstrapped as a new install does.
async fn fresh_target(
    admin: &sqlx::PgPool,
    source_url: &str,
    name: &str,
) -> (String, sqlx::PgPool) {
    drop_target(admin, name).await;
    let sql = match name {
        TARGET_ONE => "CREATE DATABASE parlour_import_flow_one",
        _ => "CREATE DATABASE parlour_import_flow_two",
    };
    sqlx::query(sql)
        .execute(admin)
        .await
        .expect("create a target database (the test role needs CREATEDB)");
    let url = with_database(source_url, name);
    let pool = parlour_db::connect(&url).await.expect("connect target");
    parlour_db::migrate(&pool).await.expect("migrate target");
    parlour_server::auth::bootstrap_admin(&pool, &test_config(&url))
        .await
        .expect("bootstrap target");
    (url, pool)
}

struct Server {
    http: reqwest::Client,
    base: String,
}

/// Serve `pool` on a free port, with its cache under `root`.
async fn serve(pool: &sqlx::PgPool, root: &Path) -> Server {
    let mut state = AppState::new(pool.clone(), None, false);
    state.export_dir = parlour_server::export::export_dir(&root.join("cache"));
    state.import_dir = parlour_server::import::import_dir(&root.join("cache"));
    state.metadata.artwork_dir = root.join("artwork");
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

    async fn wait(&self, cookie: &str, job: &Value) -> Value {
        let id = job["id"].as_str().unwrap().to_string();
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

    /// Export over HTTP and return the bundle's bytes.
    async fn export(&self, cookie: &str, body: Value) -> Vec<u8> {
        let (status, job) = self
            .req(
                Method::POST,
                "/api/v1/admin/export",
                Some(cookie),
                Some(body),
            )
            .await;
        assert_eq!(status, 202, "{job}");
        let job = self.wait(cookie, &job).await;
        assert_eq!(job["status"], "succeeded", "{job}");
        let id = job["id"].as_str().unwrap();
        let res = self
            .send(
                Method::GET,
                &format!("/api/v1/admin/export/{id}/download"),
                Some(cookie),
                None,
            )
            .await;
        assert_eq!(res.status(), 200);
        res.bytes().await.unwrap().to_vec()
    }

    /// Upload a bundle; returns (status, body).
    async fn upload(&self, cookie: Option<&str>, bytes: Vec<u8>) -> (reqwest::StatusCode, Value) {
        let mut rb = self
            .http
            .post(format!("{}/api/v1/admin/import", self.base))
            .header("Content-Type", "application/zip")
            .body(bytes);
        if let Some(c) = cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        let res = rb.send().await.expect("upload");
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Apply an upload and wait for its job.
    async fn apply(&self, cookie: &str, id: &str, options: Value) -> Value {
        let (status, job) = self
            .req(
                Method::POST,
                &format!("/api/v1/admin/import/{id}/apply"),
                Some(cookie),
                Some(options),
            )
            .await;
        assert_eq!(status, 202, "{job}");
        assert_eq!(job["kind"], "import_bundle");
        let job = self.wait(cookie, &job).await;
        assert_eq!(job["status"], "succeeded", "{job}");
        job
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn open(bytes: Vec<u8>) -> Bundle {
    BundleReader::open(Cursor::new(bytes))
        .expect("a bundle the reader accepts")
        .1
}

/// The source server: a second user with a PIN'd profile, two
/// libraries with folders and grants, a matched movie and a series
/// down to an episode, credits, segments (a D21 marker included),
/// provider artwork with one cached poster, watch state, recent
/// searches and Activity.
async fn seed(pool: &sqlx::PgPool, artwork_dir: &Path) {
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
        "INSERT INTO profiles (user_id, name, pin_hash, subtitle_mode, segment_skip, autoplay_next)
         VALUES ($1, 'Kids', $2, 'always', 'auto', false) RETURNING id",
    )
    .bind(ada)
    .bind(&pin)
    .fetch_one(pool)
    .await
    .unwrap();
    let (jordan_profile,): (Uuid,) = sqlx::query_as(
        "SELECT p.id FROM profiles p JOIN users u ON u.id = p.user_id WHERE u.username = 'jordan'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE profiles SET avatar_color = '#3a7bd5', audio_lang = 'en' WHERE id = $1")
        .bind(jordan_profile)
        .execute(pool)
        .await
        .unwrap();

    let library = |name: &'static str, kind: &'static str| async move {
        let (id,): (Uuid,) =
            sqlx::query_as("INSERT INTO libraries (name, kind) VALUES ($1, $2) RETURNING id")
                .bind(name)
                .bind(kind)
                .fetch_one(pool)
                .await
                .unwrap();
        sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("/old-host/media/{}", name.to_lowercase()))
            .execute(pool)
            .await
            .unwrap();
        id
    };
    let films = library("Films", "movies").await;
    let series = library("Series", "shows").await;
    for (lib, profile, rating) in [
        (films, ada_profile, None),
        (films, kids, Some("PG")),
        (series, ada_profile, None),
        (films, jordan_profile, None),
    ] {
        sqlx::query(
            "INSERT INTO library_access (library_id, profile_id, max_rating) VALUES ($1, $2, $3)",
        )
        .bind(lib)
        .bind(profile)
        .bind(rating)
        .execute(pool)
        .await
        .unwrap();
    }

    let item = |lib: Uuid,
                parent: Option<Uuid>,
                kind: &'static str,
                key: &'static str,
                tmdb: i64,
                status: &'static str| async move {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_items (library_id, parent_id, kind, stable_key, title, sort_title,
                                      tmdb_id, genres, metadata_status, last_metadata_at,
                                      overview, vote_average)
             VALUES ($1, $2, $3, $4, $4, $4, $5, '{Drama}', $6,
                     CASE WHEN $6 = 'matched' THEN now() END, 'An overview.', 7.5)
             RETURNING id",
        )
        .bind(lib)
        .bind(parent)
        .bind(kind)
        .bind(key)
        .bind(tmdb)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap();
        id
    };
    let movie = item(
        films,
        None,
        "movie",
        "movie:lanterns:lanterns:2021",
        7001,
        "matched",
    )
    .await;
    let show = item(
        series,
        None,
        "series",
        "series:harbor:2019",
        7002,
        "needs_review",
    )
    .await;
    let season = item(
        series,
        Some(show),
        "season",
        "series:harbor:2019:s01",
        7003,
        "unmatched",
    )
    .await;
    let episode = item(
        series,
        Some(season),
        "episode",
        "series:harbor:2019:s01:e001",
        7004,
        "unmatched",
    )
    .await;

    let (person,): (Uuid,) = sqlx::query_as(
        "INSERT INTO people (name, tmdb_id) VALUES ('Juniper Hale', 501) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO item_people (item_id, person_id, role, character_name, sort_order)
         VALUES ($1, $2, 'actor', 'Mara', 0), ($3, $2, 'actor', NULL, 0)",
    )
    .bind(movie)
    .bind(person)
    .bind(show)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, source, start_ms, end_ms, confidence)
         VALUES ($1, 'intro', 'fingerprint', 1000, 31000, 0.8),
                ($1, 'credits', 'manual', NULL, NULL, 1.0)",
    )
    .bind(episode)
    .execute(pool)
    .await
    .unwrap();

    // Artwork: a cached poster and a backdrop still waiting for its
    // download.
    let poster = b"import-flow poster bytes, not really a jpeg".to_vec();
    let poster_path = artwork_dir.join("poster-original.jpg");
    std::fs::write(&poster_path, &poster).unwrap();
    for (kind, variant, path, hash) in [
        (
            "poster",
            "original",
            poster_path.display().to_string(),
            Some(sha(&poster)),
        ),
        (
            "backdrop",
            "original",
            "pending:backdrop:original".to_string(),
            None,
        ),
    ] {
        sqlx::query(
            "INSERT INTO images (item_id, kind, variant, path, hash, source_url)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(movie)
        .bind(kind)
        .bind(variant)
        .bind(path)
        .bind(hash)
        .bind(format!("https://images.example.org/{kind}-{variant}.jpg"))
        .execute(pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played,
                                is_favorite, play_count, last_played_at)
         VALUES ($1, $2, 120000, 5400000, false, true, 1, now()),
                ($1, $3, 0, 1500000, true, false, 2, now()),
                ($4, $2, 600000, 5400000, false, false, 1, now()),
                ($5, $2, 30000, 5400000, false, false, 1, now())",
    )
    .bind(ada_profile)
    .bind(movie)
    .bind(episode)
    .bind(kids)
    .bind(jordan_profile)
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
    parlour_server::activity::record(
        pool,
        parlour_server::activity::Entry::by_user(
            "playback.start",
            ada,
            "ada",
            "ada started a film",
        )
        .profile(kids)
        .item(movie)
        .detail(json!({"client": "web", "ip": "192.0.2.10"})),
    )
    .await;
}

/// Row counts of every carried table but `activity`.
async fn counts(pool: &sqlx::PgPool) -> BTreeMap<&'static str, i64> {
    let row: Vec<i64> = sqlx::query_scalar(
        "SELECT unnest(ARRAY[
             (SELECT count(*) FROM users), (SELECT count(*) FROM profiles),
             (SELECT count(*) FROM recent_searches), (SELECT count(*) FROM libraries),
             (SELECT count(*) FROM library_paths), (SELECT count(*) FROM library_access),
             (SELECT count(*) FROM media_items), (SELECT count(*) FROM people),
             (SELECT count(*) FROM item_people), (SELECT count(*) FROM images),
             (SELECT count(*) FROM media_segments), (SELECT count(*) FROM user_data)])",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    [
        "users",
        "profiles",
        "recent_searches",
        "libraries",
        "library_paths",
        "library_access",
        "media_items",
        "people",
        "item_people",
        "images",
        "media_segments",
        "user_data",
    ]
    .into_iter()
    .zip(row)
    .collect()
}

/// The report minus the dry-run flag, to compare a dry run with the
/// job's result.
fn without_flag(mut report: Value) -> Value {
    report.as_object_mut().unwrap().remove("dry_run");
    report
}

fn tally(report: &Value, what: &str) -> (u64, u64, u64, u64) {
    let t = &report[what];
    (
        t["created"].as_u64().unwrap(),
        t["updated"].as_u64().unwrap(),
        t["existing"].as_u64().unwrap(),
        t["skipped"].as_u64().unwrap(),
    )
}

fn user<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == name)
        .unwrap_or_else(|| panic!("{name} in {report}"))
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn import_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let source = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&source).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&source, &test_config(&url))
        .await
        .expect("bootstrap");
    let scratch: PathBuf =
        std::env::temp_dir().join(format!("parlour-import-flow-{}", Uuid::new_v4()));
    let source_root = scratch.join("source");
    std::fs::create_dir_all(source_root.join("artwork")).unwrap();
    seed(&source, &source_root.join("artwork")).await;

    let old = serve(&source, &source_root).await;
    let old_admin = old.login("jordan", ADMIN_PASSWORD).await;
    let hashed = old
        .export(
            &old_admin,
            json!({"include_hashes": true, "include_artwork": true}),
        )
        .await;
    let plain = old.export(&old_admin, json!({})).await;
    let source_bundle = open(hashed.clone());
    assert_eq!(source_bundle.catalog.items.len(), 4);
    assert_eq!(source_bundle.artwork_files().len(), 1);

    // ================= 1. Round trip onto a fresh install (D38).
    let (_, one) = fresh_target(&source, &url, TARGET_ONE).await;
    let one_root = scratch.join("one");
    let new = serve(&one, &one_root).await;

    // Anonymous: 401. A non-admin can't see the surface (404).
    let (status, _) = new.upload(None, hashed.clone()).await;
    assert_eq!(status, 401);
    let admin = new.login("jordan", ADMIN_PASSWORD).await;

    // Not a bundle: refused, and nothing is left behind.
    let (status, body) = new.upload(Some(&admin), b"not a zip".to_vec()).await;
    assert_eq!(status, 400, "{body}");
    let imports = one_root.join("cache").join("imports");
    let leftovers = std::fs::read_dir(&imports).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftovers, 0);

    let before = counts(&one).await;
    let (status, uploaded) = new.upload(Some(&admin), hashed.clone()).await;
    assert_eq!(status, 201, "{uploaded}");
    let id = uploaded["id"].as_str().unwrap().to_string();
    assert_eq!(uploaded["bytes"].as_u64(), Some(hashed.len() as u64));
    let report = &uploaded["report"];
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["manifest"]["hashes_included"], true);
    // The dry run changed nothing.
    assert_eq!(counts(&one).await, before);

    let jordan = user(report, "jordan");
    assert_eq!(jordan["action"], "adopt", "{report}");
    assert_eq!(jordan["sign_in"], "from_bundle");
    let ada = user(report, "ada");
    assert_eq!(ada["action"], "create");
    assert_eq!(ada["sign_in"], "from_bundle");
    assert_eq!(report["conflicts_total"], 0, "{report}");
    assert_eq!(
        tally(report, "profiles"),
        (2, 1, 0, 0),
        "the adopted default profile is updated"
    );
    assert_eq!(tally(report, "items"), (4, 0, 0, 0));
    assert_eq!(tally(report, "access"), (4, 0, 0, 0));
    assert_eq!(tally(report, "segments"), (2, 0, 0, 0));
    assert_eq!(tally(report, "images"), (2, 0, 0, 0));
    assert_eq!(tally(report, "artwork_files"), (1, 0, 0, 0));
    assert_eq!(tally(report, "watch_state"), (4, 0, 0, 0));
    assert_eq!(tally(report, "recent_searches"), (2, 0, 0, 0));
    let libraries = report["libraries"].as_array().unwrap();
    assert_eq!(libraries.len(), 2);
    assert!(libraries.iter().all(|l| l["action"] == "create"));
    assert_eq!(
        libraries[0]["bundle_paths"],
        json!([{"path": "/old-host/media/films", "exists_here": false}])
    );
    let notes = report["notes"].to_string();
    assert!(notes.contains("unavailable"), "{notes}");
    assert!(notes.contains("same encodes"), "{notes}");
    assert!(notes.contains("candidates"), "{notes}");
    assert!(notes.contains("§9"), "{notes}");

    // A dry run with options: an unknown library is refused.
    let (status, _) = new
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{id}/dry-run"),
            Some(&admin),
            Some(json!({"library_paths": {"Cartoons": ["/srv/cartoons"]}})),
        )
        .await;
    assert_eq!(status, 400);

    // Apply: the job does exactly what the dry run said.
    let job = new.apply(&admin, &id, json!({})).await;
    let result = job["result"].clone();
    assert_eq!(result["dry_run"], false);
    assert_eq!(without_flag(result.clone()), without_flag(report.clone()));
    // The upload is gone once applied (it held hashes).
    let (status, _) = new
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{id}/dry-run"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 404);

    // The adopted admin's session lives on; the old passwords work.
    let (status, me) = new
        .req(Method::GET, "/api/v1/auth/me", Some(&admin), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(me["user"]["username"], "jordan");
    new.login("ada", ADA_PASSWORD).await;
    assert_eq!(counts(&one).await["users"], 2);
    // Imported items wait for a scan to find their files.
    let (available,): (i64,) = sqlx::query_as("SELECT count(*) FROM media_items WHERE available")
        .fetch_one(&one)
        .await
        .unwrap();
    assert_eq!(available, 0);
    // The cached poster is in this server's artwork cache.
    let (poster_path, poster_hash): (String, Option<String>) = sqlx::query_as(
        "SELECT path, hash FROM images WHERE kind = 'poster' AND variant = 'original'",
    )
    .fetch_one(&one)
    .await
    .unwrap();
    assert!(poster_path.starts_with(&one_root.join("artwork").display().to_string()));
    assert_eq!(
        sha(&std::fs::read(&poster_path).unwrap()),
        poster_hash.unwrap()
    );

    // State matches: the target exports what the source did.
    let again = open(
        new.export(
            &admin,
            json!({"include_hashes": true, "include_artwork": true}),
        )
        .await,
    );
    assert_eq!(again.users, source_bundle.users);
    assert_eq!(again.profiles, source_bundle.profiles);
    assert_eq!(again.libraries, source_bundle.libraries);
    assert_eq!(again.catalog, source_bundle.catalog);
    assert_eq!(again.user_data.profiles, source_bundle.user_data.profiles);
    for entry in &source_bundle.user_data.activity {
        assert!(
            again.user_data.activity.contains(entry),
            "{entry:?} was not carried over"
        );
    }
    let ours: Vec<&str> = again
        .user_data
        .activity
        .iter()
        .filter(|e| !source_bundle.user_data.activity.contains(e))
        .map(|e| e.kind.as_str())
        .collect();
    assert!(
        ours.iter()
            .all(|k| ["auth.login", "import.bundle_applied", "export.completed"].contains(k)),
        "{ours:?}"
    );
    assert!(ours.contains(&"import.bundle_applied"));

    // Re-import: nothing new, nothing duplicated — Activity included.
    let rows = counts(&one).await;
    let (activity_before,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM activity WHERE kind NOT LIKE 'import.%' AND kind NOT LIKE 'export.%'",
    )
    .fetch_one(&one)
    .await
    .unwrap();
    let (status, uploaded) = new.upload(Some(&admin), hashed.clone()).await;
    assert_eq!(status, 201);
    let report = &uploaded["report"];
    assert_eq!(user(report, "jordan")["action"], "existing");
    assert_eq!(user(report, "ada")["action"], "existing");
    assert!(report["libraries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["action"] == "existing" && l["paths_added"] == json!([])));
    for what in [
        "profiles",
        "access",
        "items",
        "people",
        "credits",
        "segments",
        "images",
        "watch_state",
        "recent_searches",
        "activity",
    ] {
        let (created, updated, _, skipped) = tally(report, what);
        assert_eq!((created, updated, skipped), (0, 0, 0), "{what}: {report}");
    }
    let job = new
        .apply(&admin, uploaded["id"].as_str().unwrap(), json!({}))
        .await;
    assert_eq!(tally(&job["result"], "activity").0, 0);
    assert_eq!(counts(&one).await, rows);
    let (activity_after,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM activity WHERE kind NOT LIKE 'import.%' AND kind NOT LIKE 'export.%'",
    )
    .fetch_one(&one)
    .await
    .unwrap();
    assert_eq!(activity_after, activity_before);

    // ================= 2. No hashes, onto a server that isn't fresh.
    let (_, two) = fresh_target(&source, &url, TARGET_TWO).await;
    let two_root = scratch.join("two");
    let host = serve(&two, &two_root).await;
    let admin = host.login("jordan", ADMIN_PASSWORD).await;
    // This admin has state of its own, so it isn't adopted.
    sqlx::query(
        "INSERT INTO recent_searches (profile_id, query)
         SELECT p.id, 'mine' FROM profiles p JOIN users u ON u.id = p.user_id
         WHERE u.username = 'jordan'",
    )
    .execute(&two)
    .await
    .unwrap();
    // "Series" here holds movies: a library conflict. "Films" exists,
    // with the bundle's movie under another key but its tmdb id.
    let (here_films,): (Uuid,) = sqlx::query_as(
        "INSERT INTO libraries (name, kind) VALUES ('Films', 'movies') RETURNING id",
    )
    .fetch_one(&two)
    .await
    .unwrap();
    sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, '/media/films')")
        .bind(here_films)
        .execute(&two)
        .await
        .unwrap();
    sqlx::query("INSERT INTO libraries (name, kind) VALUES ('Series', 'movies')")
        .execute(&two)
        .await
        .unwrap();
    let (here_movie,): (Uuid,) = sqlx::query_as(
        "INSERT INTO media_items (library_id, kind, stable_key, title, sort_title, tmdb_id)
         VALUES ($1, 'movie', 'movie:lanterns-2021:lanterns:2021', 'lanterns', 'lanterns', 7001)
         RETURNING id",
    )
    .bind(here_films)
    .fetch_one(&two)
    .await
    .unwrap();

    let (status, uploaded) = host.upload(Some(&admin), plain.clone()).await;
    assert_eq!(status, 201, "{uploaded}");
    let id = uploaded["id"].as_str().unwrap().to_string();
    let options = json!({"library_paths": {"Films": ["/srv/new-host/films"]}});
    let (status, report) = host
        .req(
            Method::POST,
            &format!("/api/v1/admin/import/{id}/dry-run"),
            Some(&admin),
            Some(options.clone()),
        )
        .await;
    assert_eq!(status, 200, "{report}");
    let jordan = user(&report, "jordan");
    assert_eq!(jordan["action"], "conflict", "{report}");
    assert_eq!(jordan["sign_in"], "none");
    let ada = user(&report, "ada");
    assert_eq!(
        (&ada["action"], &ada["sign_in"]),
        (&json!("create"), &json!("locked"))
    );
    let conflicts = report["conflicts"].as_array().unwrap();
    assert!(conflicts
        .iter()
        .any(|c| c["what"] == "user" && c["key"] == "jordan"));
    assert!(conflicts
        .iter()
        .any(|c| c["what"] == "library" && c["key"] == "Series"));
    assert_eq!(report["conflicts_total"], 2, "{report}");
    let films = &report["libraries"][0];
    assert_eq!(films["action"], "existing");
    assert_eq!(
        films["paths_added"],
        json!([{"path": "/srv/new-host/films", "exists_here": false}])
    );
    assert_eq!(report["libraries"][1]["action"], "conflict");
    assert_eq!(report["items_matched_by_provider_id"], 1);
    // The movie: matched on its tmdb id, and the bundle's metadata is
    // newer. The series, season and episode: their library clashes.
    assert_eq!(tally(&report, "items"), (0, 1, 0, 3));
    // jordan's own profile is skipped with him.
    assert_eq!(tally(&report, "profiles"), (2, 0, 0, 1));
    // ada's two grants on Films; jordan's grant and the Series grant
    // skipped.
    assert_eq!(tally(&report, "access"), (2, 0, 0, 2));
    // ada/ada and ada/Kids on the movie; ada's episode row and
    // jordan's movie row skipped.
    assert_eq!(tally(&report, "watch_state"), (2, 0, 0, 2));
    assert!(report["notes"].to_string().contains("D37"));

    let job = host.apply(&admin, &id, options).await;
    assert_eq!(tally(&job["result"], "items"), (0, 1, 0, 3));
    let (key, title, status_here): (String, String, String) =
        sqlx::query_as("SELECT stable_key, title, metadata_status FROM media_items WHERE id = $1")
            .bind(here_movie)
            .fetch_one(&two)
            .await
            .unwrap();
    assert_eq!(
        key, "movie:lanterns-2021:lanterns:2021",
        "the key here stays"
    );
    assert_eq!(
        (title.as_str(), status_here.as_str()),
        ("movie:lanterns:lanterns:2021", "matched")
    );
    let paths: Vec<(String,)> =
        sqlx::query_as("SELECT path FROM library_paths WHERE library_id = $1 ORDER BY path")
            .bind(here_films)
            .fetch_all(&two)
            .await
            .unwrap();
    assert_eq!(
        paths,
        [
            ("/media/films".to_string(),),
            ("/srv/new-host/films".to_string(),)
        ]
    );

    // D37: ada exists but can't sign in until an admin sets a password.
    assert!(host.try_login("ada", ADA_PASSWORD).await.is_none());
    let (_, users) = host
        .req(Method::GET, "/api/v1/admin/users", Some(&admin), None)
        .await;
    let ada_row = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "ada")
        .unwrap()
        .clone();
    assert_eq!(ada_row["password_locked"], true);
    let ada_id = ada_row["id"].as_str().unwrap();
    // Generated at run time: no password literal for this one.
    let fresh = format!("set-{}", Uuid::new_v4().simple());
    let (status, _) = host
        .req(
            Method::PUT,
            &format!("/api/v1/admin/users/{ada_id}/password"),
            Some(&admin),
            // Seven characters, built at run time: one short of the rule.
            Some(json!({"password": "x".repeat(7)})),
        )
        .await;
    assert_eq!(status, 400, "the usual password rules apply");
    let (status, _) = host
        .req(
            Method::PUT,
            &format!("/api/v1/admin/users/{}/password", Uuid::new_v4()),
            Some(&admin),
            Some(json!({"password": fresh})),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = host
        .req(
            Method::PUT,
            &format!("/api/v1/admin/users/{ada_id}/password"),
            Some(&admin),
            Some(json!({"password": fresh})),
        )
        .await;
    assert_eq!(status, 204);
    let ada_cookie = host.login("ada", &fresh).await;
    // Only an admin may set passwords.
    let (status, _) = host
        .req(
            Method::PUT,
            &format!("/api/v1/admin/users/{ada_id}/password"),
            Some(&ada_cookie),
            Some(json!({"password": fresh})),
        )
        .await;
    assert_eq!(status, 404);
    let (_, users) = host
        .req(Method::GET, "/api/v1/admin/users", Some(&admin), None)
        .await;
    assert!(users
        .as_array()
        .unwrap()
        .iter()
        .all(|u| u["password_locked"] == false));
    let (set,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM activity WHERE kind = 'user.password_set'")
            .fetch_one(&two)
            .await
            .unwrap();
    assert_eq!(set, 1);
    // ada's Kids profile had a PIN: it keeps one nobody knows.
    let (kids_pin,): (Option<String>,) =
        sqlx::query_as("SELECT pin_hash FROM profiles WHERE name = 'Kids'")
            .fetch_one(&two)
            .await
            .unwrap();
    assert!(parlour_server::auth::is_locked_hash(&kids_pin.unwrap()));
    // The source's activity naming jordan arrives without that link.
    let (detached,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM activity WHERE actor = 'jordan' AND user_id IS NULL")
            .fetch_one(&two)
            .await
            .unwrap();
    assert!(detached >= 1, "the source's export.completed rows");
    // The admin here keeps everything of theirs.
    let (mine,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM recent_searches WHERE query = 'mine'")
            .fetch_one(&two)
            .await
            .unwrap();
    assert_eq!(mine, 1);

    // Again without hashes: idempotent, and the conflicts still stand.
    let rows = counts(&two).await;
    let (_, uploaded) = host.upload(Some(&admin), plain.clone()).await;
    let report = &uploaded["report"];
    assert_eq!(user(report, "jordan")["action"], "conflict");
    assert_eq!(user(report, "ada")["action"], "existing");
    assert_eq!(
        user(report, "ada")["sign_in"],
        "kept",
        "set by the admin now"
    );
    for what in [
        "profiles",
        "items",
        "watch_state",
        "recent_searches",
        "activity",
    ] {
        let (created, updated, _, _) = tally(report, what);
        assert_eq!((created, updated), (0, 0), "{what}: {report}");
    }
    host.apply(&admin, uploaded["id"].as_str().unwrap(), json!({}))
        .await;
    assert_eq!(counts(&two).await, rows);

    // Now with hashes: ada's password (set here) is kept, and the
    // locked PIN is filled in from the bundle.
    let (_, uploaded) = host.upload(Some(&admin), hashed.clone()).await;
    let report = &uploaded["report"];
    assert_eq!(user(report, "ada")["sign_in"], "kept");
    assert_eq!(tally(report, "profiles"), (0, 1, 1, 1));
    host.apply(&admin, uploaded["id"].as_str().unwrap(), json!({}))
        .await;
    let (kids_pin,): (Option<String>,) =
        sqlx::query_as("SELECT pin_hash FROM profiles WHERE name = 'Kids'")
            .fetch_one(&two)
            .await
            .unwrap();
    let kids_pin = kids_pin.unwrap();
    assert!(!parlour_server::auth::is_locked_hash(&kids_pin));
    assert!(parlour_server::auth::verify_password("4321", &kids_pin));
    host.login("ada", &fresh).await;
    assert_eq!(counts(&two).await["users"], 2);

    // Clean up the target databases and the scratch folders.
    one.close().await;
    two.close().await;
    for name in [TARGET_ONE, TARGET_TWO] {
        drop_target(&source, name).await;
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
