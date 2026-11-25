//! End-to-end server export against a real Postgres (XFER2): an admin
//! queues an export, the job writes a `parlour-export` v1 bundle of
//! the seeded database, and the download serves it. Covers the hash
//! confirm off and on, the artwork opt-in off and on (with the size
//! estimate), what a non-admin sees, delete, and the rows an export
//! can't carry as they stand (a stale duplicate image, a cached file
//! that is gone).
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a fresh
//! throwaway database and run `cargo test -- --ignored`. The database
//! is migrated by the server code itself. Every password, PIN and
//! hash here is a throwaway made up for this test.

use std::io::Cursor;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use parlour_server::bundle::{BundleReader, SegmentSource};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use reqwest::Method;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const ADMIN_PASSWORD: &str = "admin-pass-123";
// Reuses the admin fixture string: GitGuardian false-positived on the old value; this one already passes in this file.
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

struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
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

    async fn login(&self, username: &str, password: &str) -> String {
        let res = self
            .send(
                Method::POST,
                "/api/v1/auth/login",
                None,
                Some(json!({"username": username, "password": password})),
            )
            .await;
        assert_eq!(res.status(), 200, "login {username}");
        res.headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(';').next())
            .and_then(|kv| kv.split_once('='))
            .map(|(_, v)| v.to_string())
            .expect("session cookie")
    }

    /// Queue an export and wait for its job to finish.
    async fn export(&self, cookie: &str, body: Option<Value>) -> Value {
        let (status, job) = self
            .req(Method::POST, "/api/v1/admin/export", Some(cookie), body)
            .await;
        assert_eq!(status, 202, "{job}");
        assert_eq!(job["kind"], "export_bundle");
        let id = job["id"].as_str().unwrap().to_string();
        for _ in 0..200 {
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
        panic!("export {id} did not finish");
    }

    async fn download(&self, cookie: &str, job: &Value) -> Vec<u8> {
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
        let header = |name: &str| {
            res.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(header("content-type"), "application/zip");
        assert_eq!(header("cache-control"), "no-store");
        let file_name = job["result"]["file_name"].as_str().unwrap();
        assert!(file_name.starts_with("parlour-export-") && file_name.ends_with(".zip"));
        assert_eq!(
            header("content-disposition"),
            format!("attachment; filename=\"{file_name}\"")
        );
        let bytes = res.bytes().await.unwrap().to_vec();
        assert_eq!(job["result"]["bytes"].as_u64(), Some(bytes.len() as u64));
        bytes
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The seeded server's ids the test needs again.
struct Seeded {
    poster: Vec<u8>,
    still_path: String,
    episode: Uuid,
}

/// One user besides the admin, a PIN'd second profile, two libraries
/// with paths and grants, a movie and a series down to an episode,
/// credits, segments (a D21 marker included), provider artwork with
/// one cached poster, watch state, recent searches and Activity.
async fn seed(pool: &sqlx::PgPool, artwork_dir: &std::path::Path) -> Seeded {
    let ada_user = parlour_server::auth::create_user(pool, "ada", ADA_PASSWORD, false)
        .await
        .expect("ada");
    let (ada_profile,): (Uuid,) = sqlx::query_as("SELECT id FROM profiles WHERE user_id = $1")
        .bind(ada_user)
        .fetch_one(pool)
        .await
        .unwrap();
    let pin = parlour_server::auth::hash_password("4321").unwrap();
    let (kids,): (Uuid,) = sqlx::query_as(
        "INSERT INTO profiles (user_id, name, pin_hash, subtitle_mode, segment_skip, autoplay_next)
         VALUES ($1, 'Kids', $2, 'always', 'auto', false) RETURNING id",
    )
    .bind(ada_user)
    .bind(&pin)
    .fetch_one(pool)
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
            .bind(format!("/media/{}", name.to_lowercase()))
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

    let item = |lib: Uuid, parent: Option<Uuid>, kind: &'static str, key: &'static str| async move {
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO media_items (library_id, parent_id, kind, stable_key, title, sort_title,
                                      tmdb_id, genres, metadata_status)
             VALUES ($1, $2, $3, $4, $4, $4, 7, '{Drama}', 'matched') RETURNING id",
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
    let show = item(series, None, "series", "series:harbor:2019").await;
    let season = item(series, Some(show), "season", "series:harbor:2019:s01").await;
    let episode = item(
        series,
        Some(season),
        "episode",
        "series:harbor:2019:s01:e001",
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

    // Artwork: a cached poster (two variants, one file), a backdrop
    // still waiting for its download, and an episode still whose
    // cached file has gone missing.
    let poster = b"export-flow poster bytes, not really a jpeg".to_vec();
    let poster_path = artwork_dir.join("poster-original.jpg");
    std::fs::write(&poster_path, &poster).unwrap();
    // The w300 download came back byte-identical: two cached files,
    // one name in the bundle.
    let small_path = artwork_dir.join("poster-w300.jpg");
    std::fs::write(&small_path, &poster).unwrap();
    let still_path = artwork_dir.join("still-gone.png").display().to_string();
    for (item_id, kind, variant, path, hash) in [
        (
            movie,
            "poster",
            "original",
            poster_path.display().to_string(),
            Some(sha(&poster)),
        ),
        (
            movie,
            "poster",
            "w300",
            small_path.display().to_string(),
            Some(sha(&poster)),
        ),
        (
            movie,
            "backdrop",
            "original",
            "pending:backdrop:original".to_string(),
            None,
        ),
        (
            episode,
            "still",
            "original",
            still_path.clone(),
            Some(sha(b"gone")),
        ),
    ] {
        sqlx::query(
            "INSERT INTO images (item_id, kind, variant, path, hash, source_url)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(item_id)
        .bind(kind)
        .bind(variant)
        .bind(path)
        .bind(hash)
        .bind(format!("https://images.example.org/{kind}-{variant}.jpg"))
        .execute(pool)
        .await
        .unwrap();
    }
    // A generated tile (no source_url) never travels.
    sqlx::query(
        "INSERT INTO images (item_id, kind, path) VALUES ($1, 'poster', '/artwork/tile.svg')",
    )
    .bind(season)
    .execute(pool)
    .await
    .unwrap();

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
    parlour_server::activity::record(
        pool,
        parlour_server::activity::Entry::by_user(
            "playback.start",
            ada_user,
            "ada",
            "ada started a film",
        )
        .profile(kids)
        .item(movie)
        .detail(json!({"client": "web", "ip": "192.0.2.10"})),
    )
    .await;

    Seeded {
        poster,
        still_path,
        episode,
    }
}

fn open(
    bytes: Vec<u8>,
) -> (
    BundleReader<Cursor<Vec<u8>>>,
    parlour_server::bundle::Bundle,
) {
    BundleReader::open(Cursor::new(bytes)).expect("the download is a bundle the reader accepts")
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn export_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let scratch: PathBuf =
        std::env::temp_dir().join(format!("parlour-export-flow-{}", Uuid::new_v4()));
    let artwork_dir = scratch.join("artwork");
    std::fs::create_dir_all(&artwork_dir).unwrap();
    let export_dir = parlour_server::export::export_dir(&scratch.join("cache"));
    let seeded = seed(&pool, &artwork_dir).await;

    let mut state = AppState::new(pool.clone(), None, false);
    state.export_dir = export_dir.clone();
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
    let client = Client {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    };

    // Anonymous: 401. A non-admin: the admin surface doesn't exist.
    let (status, _) = client
        .req(Method::POST, "/api/v1/admin/export", None, None)
        .await;
    assert_eq!(status, 401);
    let ada = client.login("ada", ADA_PASSWORD).await;
    for (method, path) in [
        (Method::POST, "/api/v1/admin/export"),
        (Method::GET, "/api/v1/admin/export/estimate"),
    ] {
        let (status, _) = client.req(method, path, Some(&ada), None).await;
        assert_eq!(status, 404, "{path} for a non-admin");
    }
    let admin = client.login("jordan", ADMIN_PASSWORD).await;

    // The size warning's numbers: one distinct file (the two poster
    // variants hold the same bytes); the still's file is gone.
    let (status, estimate) = client
        .req(
            Method::GET,
            "/api/v1/admin/export/estimate",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        estimate,
        json!({
            "artwork_files": 1,
            "artwork_bytes": seeded.poster.len(),
            "artwork_unavailable": 1,
        })
    );

    // A misspelt option is refused, not ignored.
    let (status, _) = client
        .req(
            Method::POST,
            "/api/v1/admin/export",
            Some(&admin),
            Some(json!({"include_hash": true})),
        )
        .await;
    assert!(status.is_client_error(), "{status}");

    // ---- Default export: an empty body means no hashes, no artwork.
    let plain = client.export(&admin, None).await;
    assert_eq!(plain["status"], "succeeded", "{plain}");
    assert_eq!(plain["payload"]["include_hashes"], false);
    let result = &plain["result"];
    assert_eq!(result["hashes_included"], false);
    assert_eq!(result["artwork_included"], false);
    assert_eq!(result["users"], 2);
    assert_eq!(result["profiles"], 3);
    assert_eq!(result["libraries"], 2);
    assert_eq!(result["items"], 4);
    assert_eq!(result["watch_state"], 3);
    assert_eq!(result["recent_searches"], 2);
    assert_eq!(result["artwork_files"], 0);
    let (_, bundle) = open(client.download(&admin, &plain).await);
    assert!(!bundle.manifest.hashes_included && !bundle.manifest.artwork_included);
    assert_eq!(bundle.manifest.server_version, env!("CARGO_PKG_VERSION"));
    let names: Vec<_> = bundle
        .users
        .users
        .iter()
        .map(|u| u.username.as_str())
        .collect();
    assert_eq!(names, ["ada", "jordan"]);
    assert!(bundle
        .users
        .users
        .iter()
        .all(|u| u.password_hash.is_none() && u.pin_hashes.is_none()));
    let profiles: Vec<_> = bundle
        .profiles
        .profiles
        .iter()
        .map(|p| (p.username.as_str(), p.name.as_str(), p.has_pin))
        .collect();
    assert_eq!(
        profiles,
        [
            ("ada", "ada", false),
            ("ada", "Kids", true),
            ("jordan", "jordan", false)
        ]
    );
    let searches: Vec<_> = bundle.profiles.profiles[0]
        .recent_searches
        .iter()
        .map(|s| s.query.as_str())
        .collect();
    assert_eq!(searches, ["lanterns", "harbor"]);
    let films = &bundle.libraries.libraries[0];
    assert_eq!(films.name, "Films");
    assert_eq!(films.paths, ["/media/films"]);
    assert_eq!(films.access.len(), 2);
    // By username, then profile name, byte by byte: "Kids" < "ada".
    assert_eq!(films.access[0].profile, "Kids");
    assert_eq!(films.access[0].max_rating.as_deref(), Some("PG"));
    let keys: Vec<_> = bundle
        .catalog
        .items
        .iter()
        .map(|i| i.stable_key.as_str())
        .collect();
    assert_eq!(
        keys,
        [
            "movie:lanterns:lanterns:2021",
            "series:harbor:2019",
            "series:harbor:2019:s01",
            "series:harbor:2019:s01:e001",
        ]
    );
    let movie = &bundle.catalog.items[0];
    assert_eq!(movie.people[0].name, "Juniper Hale");
    assert_eq!(movie.people[0].tmdb_id, Some(501));
    assert_eq!(movie.images.len(), 3, "provider references always travel");
    assert!(movie.images.iter().all(|i| i.file.is_none()));
    assert!(
        bundle.catalog.items[2].images.is_empty(),
        "the generated tile stays behind"
    );
    let episode = &bundle.catalog.items[3];
    assert_eq!(episode.parent.as_deref(), Some("series:harbor:2019:s01"));
    let marker = episode
        .segments
        .iter()
        .find(|s| s.source == SegmentSource::Manual)
        .unwrap();
    assert!(
        marker.start_ms.is_none() && marker.end_ms.is_none(),
        "the D21 marker"
    );
    let watch: Vec<_> = bundle
        .user_data
        .profiles
        .iter()
        .map(|g| (g.profile.as_str(), g.items.len()))
        .collect();
    assert_eq!(watch, [("Kids", 1), ("ada", 2)]);
    let start = bundle
        .user_data
        .activity
        .iter()
        .find(|a| a.kind == "playback.start")
        .unwrap();
    assert_eq!(start.username.as_deref(), Some("ada"));
    assert_eq!(start.profile.as_ref().unwrap().profile, "Kids");
    assert_eq!(
        start.item.as_ref().unwrap().stable_key,
        "movie:lanterns:lanterns:2021"
    );
    assert_eq!(
        start.detail["ip"], "192.0.2.10",
        "detail travels verbatim (D25)"
    );

    // The export is on the Activity feed, saying hashes stayed home.
    let (_, feed) = client
        .req(
            Method::GET,
            "/api/v1/admin/activity?category=export",
            Some(&admin),
            None,
        )
        .await;
    let entry = &feed["entries"][0];
    assert_eq!(entry["kind"], "export.completed");
    assert_eq!(entry["actor"], "jordan");
    assert_eq!(entry["summary"], "Server export finished");
    assert_eq!(entry["detail"]["hashes_included"], false);

    // ---- With the confirm and the artwork opt-in.
    let full = client
        .export(
            &admin,
            Some(json!({"include_hashes": true, "include_artwork": true})),
        )
        .await;
    assert_eq!(full["status"], "succeeded", "{full}");
    let result = &full["result"];
    assert_eq!(result["hashes_included"], true);
    assert_eq!(result["artwork_included"], true);
    assert_eq!(result["artwork_files"], 1);
    assert_eq!(result["artwork_left_out"], 1);
    let errors = full["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["path"], seeded.still_path.as_str());
    assert!(errors[0]["message"]
        .as_str()
        .unwrap()
        .starts_with("artwork left out of the bundle"));
    let (mut reader, bundle) = open(client.download(&admin, &full).await);
    assert!(bundle.manifest.hashes_included && bundle.manifest.artwork_included);
    let stored: Vec<(String, String)> =
        sqlx::query_as("SELECT username, password_hash FROM users ORDER BY username")
            .fetch_all(&pool)
            .await
            .unwrap();
    for (user, (username, hash)) in bundle.users.users.iter().zip(&stored) {
        assert_eq!(&user.username, username);
        assert!(
            user.password_hash.as_deref() == Some(hash.as_str()),
            "{username}'s hash as stored"
        );
    }
    let pins = bundle.users.users[0].pin_hashes.as_ref().unwrap();
    assert_eq!(pins.keys().collect::<Vec<_>>(), ["Kids"]);
    assert!(parlour_server::auth::verify_password("4321", &pins["Kids"]));
    assert!(bundle.users.users[1]
        .pin_hashes
        .as_ref()
        .unwrap()
        .is_empty());
    let poster_name = format!("artwork/{}.jpg", sha(&seeded.poster));
    assert_eq!(
        reader.artwork_names().collect::<Vec<_>>(),
        [poster_name.as_str()]
    );
    let files: Vec<_> = bundle.catalog.items[0]
        .images
        .iter()
        .map(|i| i.file.as_deref())
        .collect();
    assert_eq!(
        files,
        [Some(poster_name.as_str()), Some(poster_name.as_str()), None]
    );
    assert!(
        bundle.catalog.items[3].images[0].file.is_none(),
        "its file is gone"
    );
    let mut out = Vec::new();
    reader.copy_artwork(&poster_name, &mut out).unwrap();
    assert_eq!(out, seeded.poster);
    let (_, feed) = client
        .req(
            Method::GET,
            "/api/v1/admin/activity?category=export",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(
        feed["entries"][0]["summary"],
        "Server export finished, with password and PIN hashes"
    );

    // ---- Two exports of an unchanged server hold the same documents
    // (the Activity feed grows by the exports themselves).
    let again = client.export(&admin, None).await;
    let (_, second) = open(client.download(&admin, &again).await);
    let (_, first) = open(client.download(&admin, &plain).await);
    assert_eq!(second.users, first.users);
    assert_eq!(second.profiles, first.profiles);
    assert_eq!(second.libraries, first.libraries);
    assert_eq!(second.catalog, first.catalog);
    assert_eq!(second.user_data.profiles, first.user_data.profiles);

    // ---- A stale duplicate image row doesn't sink the export.
    sqlx::query(
        "INSERT INTO images (item_id, kind, variant, path, source_url)
         VALUES ($1, 'still', 'original', 'pending:still:original', 'https://images.example.org/still-original.jpg')",
    )
    .bind(seeded.episode)
    .execute(&pool)
    .await
    .unwrap();
    let dup = client.export(&admin, None).await;
    assert_eq!(dup["status"], "succeeded", "{dup}");
    assert_eq!(dup["result"]["duplicate_images_left_out"], 1);
    assert!(dup["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("a second still image 'original'"));
    let (_, bundle) = open(client.download(&admin, &dup).await);
    assert_eq!(bundle.catalog.items[3].images.len(), 1);

    // ---- Downloads and deletes only for finished exports.
    let queued = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::ExportBundle,
        None,
        json!({"trigger": "manual"}),
    )
    .await
    .unwrap();
    let (status, _) = client
        .req(
            Method::GET,
            &format!("/api/v1/admin/export/{queued}/download"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 409, "not finished yet");
    let other = parlour_jobs::enqueue(&pool, parlour_jobs::JobKind::PurgeActivity, None, json!({}))
        .await
        .unwrap();
    for id in [other.to_string(), Uuid::new_v4().to_string()] {
        let (status, _) = client
            .req(
                Method::GET,
                &format!("/api/v1/admin/export/{id}/download"),
                Some(&admin),
                None,
            )
            .await;
        assert_eq!(status, 404, "not an export");
    }
    let (status, _) = client
        .req(
            Method::GET,
            "/api/v1/admin/export/not-a-uuid/download",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 400);
    let full_id = full["id"].as_str().unwrap();
    let (status, _) = client
        .req(
            Method::GET,
            &format!("/api/v1/admin/export/{full_id}/download"),
            Some(&ada),
            None,
        )
        .await;
    assert_eq!(status, 404, "a non-admin can't download");

    // The worker picks up an export left queued.
    parlour_server::export::spawn_worker(pool.clone(), export_dir.clone());
    let mut finished = None;
    for _ in 0..100 {
        let job = parlour_jobs::get(&pool, queued).await.unwrap().unwrap();
        if job.status == "succeeded" || job.status == "failed" {
            finished = Some(job);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let finished = finished.expect("the worker ran the queued export");
    assert_eq!(finished.status, "succeeded");
    assert!(export_dir.join(format!("{queued}.zip")).exists());

    // Delete: the file goes, the job says so, the download is a 404.
    let path = format!("/api/v1/admin/export/{full_id}");
    let (status, _) = client.req(Method::DELETE, &path, Some(&ada), None).await;
    assert_eq!(status, 404, "a non-admin can't delete");
    let (status, _) = client.req(Method::DELETE, &path, Some(&admin), None).await;
    assert_eq!(status, 204);
    assert!(!export_dir.join(format!("{full_id}.zip")).exists());
    let (_, job) = client
        .req(
            Method::GET,
            &format!("/api/v1/admin/jobs/{full_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(job["result"]["deleted"], true);
    assert_eq!(job["result"]["file_name"], full["result"]["file_name"]);
    let (status, _) = client
        .req(Method::GET, &format!("{path}/download"), Some(&admin), None)
        .await;
    assert_eq!(status, 404);
    let (status, _) = client.req(Method::DELETE, &path, Some(&admin), None).await;
    assert_eq!(status, 404);

    // Nothing half-written was left in the export folder.
    for entry in std::fs::read_dir(&export_dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(name.ends_with(".zip") && !name.ends_with(".part"), "{name}");
    }
    std::fs::remove_dir_all(&scratch).unwrap();
}
