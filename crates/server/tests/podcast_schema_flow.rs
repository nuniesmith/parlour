//! End-to-end test of POD1 (the podcast schema, D29/D31/D32): the
//! migration applies to a fresh database; `podcasts` and
//! `podcast_episodes` have the columns, defaults and keys the spec
//! promises (docs/AUDIOBOOKS-PODCASTS.md "Schema (POD1)"); a profile
//! reaches shows and episodes only through its library grant (no
//! subscriptions table); and episode progress lives in `book_progress`
//! beside the books, one row per profile and episode, taking AB11's
//! write rules (position, auto-finish, un-finish, the hide flag, the
//! client's timestamp) — while the book endpoints never see an
//! episode's row.
//!
//! POD1 builds no API: the shows and episodes are written with SQL, as
//! POD2/POD3 will write them.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database).
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_schema_flow -- --ignored

use std::collections::BTreeMap;
use std::net::SocketAddr;

use chrono::{DateTime, TimeZone, Utc};
use parlour_server::audiobooks::{
    apply_write, clamp_position, FinishRule, FinishedAt, ProgressWrite, StoredProgress,
};
use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct TestApp {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl TestApp {
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut builder = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .header(COOKIE, &self.cookie);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let resp = builder.send().await.expect("request");
        let status = resp.status().as_u16();
        let value = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn library(&self, name: &str, kind: &str) -> Uuid {
        let (status, lib) = self
            .req(
                Method::POST,
                "/api/v1/admin/libraries",
                Some(json!({"name": name, "kind": kind, "paths": []})),
            )
            .await;
        assert_eq!(status, 201, "{lib}");
        Uuid::parse_str(lib["id"].as_str().unwrap()).unwrap()
    }

    async fn grant(&self, library: Uuid, profile: Uuid) {
        let (status, v) = self
            .req(
                Method::POST,
                &format!("/api/v1/admin/libraries/{library}/access"),
                Some(json!({"profile_id": profile})),
            )
            .await;
        assert_eq!(status, 204, "{v}");
    }
}

/// (data type, nullable, default) per column of `table`.
async fn columns(pool: &PgPool, table: &str) -> BTreeMap<String, (String, bool, Option<String>)> {
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT column_name::text, data_type::text, is_nullable::text, column_default::text
         FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("columns");
    rows.into_iter()
        .map(|(name, ty, nullable, default)| (name, (ty, nullable == "YES", default)))
        .collect()
}

/// Every column of `table` with its type and nullability, exactly.
fn assert_shape(
    table: &str,
    actual: &BTreeMap<String, (String, bool, Option<String>)>,
    expected: &[(&str, &str, bool)],
) {
    let got: BTreeMap<&str, (&str, bool)> = actual
        .iter()
        .map(|(name, (ty, nullable, _))| (name.as_str(), (ty.as_str(), *nullable)))
        .collect();
    let want: BTreeMap<&str, (&str, bool)> = expected
        .iter()
        .map(|(name, ty, nullable)| (*name, (*ty, *nullable)))
        .collect();
    assert_eq!(got, want, "{table}: columns (type, nullable)");
}

fn default_of<'a>(
    columns: &'a BTreeMap<String, (String, bool, Option<String>)>,
    column: &str,
) -> &'a str {
    columns[column].2.as_deref().unwrap_or("")
}

/// The SQLSTATE a statement failed with.
fn code(e: sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(db) => db.code().map(|c| c.to_string()).unwrap_or_default(),
        other => panic!("not a database error: {other}"),
    }
}

const UNIQUE_VIOLATION: &str = "23505";
const CHECK_VIOLATION: &str = "23514";

/// What the grant path shows a profile: the episodes of every show in
/// the podcasts libraries its `library_access` rows reach (D31).
async fn visible_episodes(pool: &PgPool, profile: Uuid) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT e.guid
         FROM podcast_episodes e
         JOIN podcasts p ON p.id = e.podcast_id
         JOIN libraries l ON l.id = p.library_id AND l.kind = 'podcasts'
         JOIN library_access a ON a.library_id = p.library_id AND a.profile_id = $1
         ORDER BY e.published_at DESC NULLS LAST, e.guid",
    )
    .bind(profile)
    .fetch_all(pool)
    .await
    .expect("visible episodes");
    rows.into_iter().map(|(g,)| g).collect()
}

#[derive(Debug, sqlx::FromRow)]
struct EpisodeProgress {
    item_id: Option<Uuid>,
    position_ms: i64,
    duration_ms: Option<i64>,
    finished: bool,
    finished_at: Option<DateTime<Utc>>,
    last_listened_at: Option<DateTime<Utc>>,
    hidden: bool,
    client_updated_at: Option<DateTime<Utc>>,
}

/// One episode progress write the way POD5 will make it: AB11's
/// `apply_write` against the stored row, the episode's downloaded
/// length and its podcasts library's finish rule, then an upsert keyed
/// on (profile, episode) — the book write with `episode_id` in place of
/// `item_id`.
async fn write_episode(
    pool: &PgPool,
    profile: Uuid,
    episode: Uuid,
    write: ProgressWrite,
    client_updated_at: Option<DateTime<Utc>>,
) -> EpisodeProgress {
    let mut tx = pool.begin().await.unwrap();
    let (duration, remaining_s, percent): (Option<i64>, i32, Option<i32>) = sqlx::query_as(
        "SELECT COALESCE(e.file_duration_ms, e.duration_ms),
                l.finish_remaining_seconds, l.finish_percent
         FROM podcast_episodes e
         JOIN podcasts p ON p.id = e.podcast_id
         JOIN libraries l ON l.id = p.library_id
         WHERE e.id = $1",
    )
    .bind(episode)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let duration = duration.filter(|d| *d > 0);
    let rule = FinishRule {
        remaining_ms: i64::from(remaining_s) * 1000,
        percent,
    };
    let stored: Option<(i64, bool, bool)> = sqlx::query_as(
        "SELECT position_ms, finished, hidden FROM book_progress
         WHERE profile_id = $1 AND episode_id = $2 FOR UPDATE",
    )
    .bind(profile)
    .bind(episode)
    .fetch_optional(&mut *tx)
    .await
    .unwrap();
    let stored = stored
        .map(|(position_ms, finished, hidden)| StoredProgress {
            position_ms,
            finished,
            hidden,
        })
        .unwrap_or_default();
    let write = ProgressWrite {
        position_ms: write.position_ms.map(|p| clamp_position(p, duration)),
        ..write
    };
    let next = apply_write(stored, write, duration, rule);
    let finished_at = match next.finished_at {
        FinishedAt::Keep => "keep",
        FinishedAt::Now => "now",
        FinishedAt::Clear => "clear",
    };
    let row = sqlx::query_as::<_, EpisodeProgress>(
        "INSERT INTO book_progress
             (profile_id, episode_id, position_ms, duration_ms, finished, finished_at,
              last_listened_at, updated_at, hidden, client_updated_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 = 'now' THEN now() END,
                 CASE WHEN $7 THEN now() END, now(), $8, COALESCE($9, now()))
         ON CONFLICT (profile_id, episode_id) DO UPDATE SET
             position_ms = $3,
             duration_ms = COALESCE($4, book_progress.duration_ms),
             finished = $5,
             finished_at = CASE $6 WHEN 'now' THEN now()
                                   WHEN 'clear' THEN NULL
                                   ELSE book_progress.finished_at END,
             last_listened_at = CASE WHEN $7 THEN now()
                                     ELSE book_progress.last_listened_at END,
             updated_at = now(),
             hidden = $8,
             client_updated_at = COALESCE($9, now())
         RETURNING item_id, position_ms, duration_ms, finished, finished_at,
                   last_listened_at, hidden, client_updated_at",
    )
    .bind(profile)
    .bind(episode)
    .bind(next.position_ms)
    .bind(duration)
    .bind(next.finished)
    .bind(finished_at)
    .bind(next.listened)
    .bind(next.hidden)
    .bind(client_updated_at)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    row
}

async fn count(pool: &PgPool, sql: &'static str, id: Uuid) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sql).bind(id).fetch_one(pool).await.unwrap();
    n
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn podcast_schema_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    // ----------------------------------- the migration on a fresh database
    parlour_db::migrate(&pool).await.expect("migrate");
    let (applied,): (bool,) =
        sqlx::query_as("SELECT success FROM _sqlx_migrations WHERE description = 'pod1 podcasts'")
            .fetch_one(&pool)
            .await
            .expect("POD1's migration is in the ledger");
    assert!(applied);

    // ------------------------------------------------------ table shapes
    let shows = columns(&pool, "podcasts").await;
    assert_shape(
        "podcasts",
        &shows,
        &[
            ("id", "uuid", false),
            ("library_id", "uuid", false),
            ("feed_url", "text", false),
            ("title", "text", false),
            ("sort_title", "text", false),
            ("author", "text", true),
            ("description", "text", true),
            ("language", "text", true),
            ("explicit", "boolean", false),
            ("podcast_type", "text", true),
            ("genres", "ARRAY", false),
            ("itunes_id", "bigint", true),
            ("image_url", "text", true),
            ("image_path", "text", true),
            ("folder_path", "text", true),
            ("etag", "text", true),
            ("last_modified", "text", true),
            ("last_checked_at", "timestamp with time zone", true),
            ("last_error", "text", true),
            ("consecutive_failures", "integer", false),
            ("auto_download", "boolean", false),
            ("check_schedule", "text", false),
            ("max_episodes_to_keep", "integer", false),
            ("max_new_episodes_per_check", "integer", false),
            ("created_at", "timestamp with time zone", false),
            ("updated_at", "timestamp with time zone", false),
        ],
    );
    // D32's defaults, from the column definitions themselves.
    assert_eq!(default_of(&shows, "auto_download"), "false");
    assert_eq!(default_of(&shows, "check_schedule"), "'0 * * * *'::text");
    assert_eq!(default_of(&shows, "max_episodes_to_keep"), "0");
    assert_eq!(default_of(&shows, "max_new_episodes_per_check"), "3");
    assert_eq!(default_of(&shows, "consecutive_failures"), "0");

    let episodes = columns(&pool, "podcast_episodes").await;
    assert_shape(
        "podcast_episodes",
        &episodes,
        &[
            ("id", "uuid", false),
            ("podcast_id", "uuid", false),
            ("guid", "text", false),
            ("title", "text", false),
            ("subtitle", "text", true),
            ("description", "text", true),
            ("published_at", "timestamp with time zone", true),
            ("duration_ms", "bigint", true),
            ("season", "integer", true),
            ("episode_number", "integer", true),
            ("episode_type", "text", true),
            ("enclosure_url", "text", false),
            ("enclosure_type", "text", true),
            ("enclosure_length", "bigint", true),
            ("chapters_url", "text", true),
            ("transcript_url", "text", true),
            ("transcript_type", "text", true),
            ("file_path", "text", true),
            ("file_size_bytes", "bigint", true),
            ("file_duration_ms", "bigint", true),
            ("file_container", "text", true),
            ("file_codec", "text", true),
            ("downloaded_at", "timestamp with time zone", true),
            ("created_at", "timestamp with time zone", false),
            ("updated_at", "timestamp with time zone", false),
        ],
    );

    let progress = columns(&pool, "book_progress").await;
    assert_shape(
        "book_progress",
        &progress,
        &[
            ("profile_id", "uuid", false),
            ("item_id", "uuid", true),
            ("episode_id", "uuid", true),
            ("position_ms", "bigint", false),
            ("duration_ms", "bigint", true),
            ("finished", "boolean", false),
            ("finished_at", "timestamp with time zone", true),
            ("last_listened_at", "timestamp with time zone", true),
            ("created_at", "timestamp with time zone", false),
            ("updated_at", "timestamp with time zone", false),
            ("hidden", "boolean", false),
            ("client_updated_at", "timestamp with time zone", true),
        ],
    );
    let constraints: Vec<(String,)> = sqlx::query_as(
        "SELECT c.conname::text FROM pg_constraint c
         JOIN pg_class r ON r.oid = c.conrelid
         WHERE r.relname = 'book_progress' AND c.contype IN ('p', 'u', 'c')
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let constraints: Vec<&str> = constraints.iter().map(|(c,)| c.as_str()).collect();
    for name in [
        "book_progress_one_target_check",
        "book_progress_profile_episode_key",
        "book_progress_profile_item_key",
    ] {
        assert!(constraints.contains(&name), "{name} in {constraints:?}");
    }
    assert!(
        !constraints.contains(&"book_progress_pkey"),
        "the (profile, item) key is a unique constraint now: {constraints:?}"
    );
    // D31: no subscriptions table, no per-profile follow.
    let (subscriptions,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables
                        WHERE table_schema = 'public' AND table_name ILIKE '%subscri%')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!subscriptions, "D31: no subscriptions table");

    // ---------------------------------------------- server, two profiles
    let mut vars = std::collections::HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), db_url.clone());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    let cfg = parlour_server::config::Config::from_vars(&vars).expect("config");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");
    let state = parlour_server::http::AppState::new(pool.clone(), None, false);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status().as_u16(), 200);
    let cookie = resp
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let app = TestApp {
        base: base.clone(),
        client,
        cookie,
    };
    let (_, profiles) = app.req(Method::GET, "/api/v1/profiles", None).await;
    let ada = Uuid::parse_str(profiles[0]["id"].as_str().unwrap()).unwrap();
    let (status, kid) = app
        .req(
            Method::POST,
            "/api/v1/profiles",
            Some(json!({"name": "Kid"})),
        )
        .await;
    assert_eq!(status, 201, "{kid}");
    let kid = Uuid::parse_str(kid["id"].as_str().unwrap()).unwrap();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{ada}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    // The podcast store is a podcasts library (D31/D32); the flow gives it
    // no folder, since POD1 writes nothing to disk.
    let pods = app.library("Podcasts", "podcasts").await;
    let pods_two = app.library("More Podcasts", "podcasts").await;
    let books = app.library("Audiobooks", "audiobooks").await;
    app.grant(pods, ada).await;
    app.grant(books, ada).await;

    // ------------------------------------------------ a show, and its defaults
    #[derive(sqlx::FromRow)]
    struct Show {
        id: Uuid,
        auto_download: bool,
        check_schedule: String,
        max_episodes_to_keep: i32,
        max_new_episodes_per_check: i32,
        consecutive_failures: i32,
        explicit: bool,
        genres: Vec<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        last_checked_at: Option<DateTime<Utc>>,
        folder_path: Option<String>,
    }
    let feed = "https://feeds.example.test/the-show.xml";
    let show = sqlx::query_as::<_, Show>(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title)
         VALUES ($1, $2, 'The Show', 'Show')
         RETURNING id, auto_download, check_schedule, max_episodes_to_keep,
                   max_new_episodes_per_check, consecutive_failures, explicit, genres,
                   etag, last_modified, last_checked_at, folder_path",
    )
    .bind(pods)
    .bind(feed)
    .fetch_one(&pool)
    .await
    .expect("insert show");
    assert!(!show.auto_download, "D32: auto-download off by default");
    assert_eq!(show.check_schedule, "0 * * * *", "D32: hourly");
    assert_eq!(show.max_episodes_to_keep, 0, "D32: 0 = keep all");
    assert_eq!(show.max_new_episodes_per_check, 3, "D32: 3 per check");
    assert_eq!(show.consecutive_failures, 0);
    assert!(!show.explicit);
    assert!(show.genres.is_empty());
    assert!(show.etag.is_none() && show.last_modified.is_none());
    assert!(show.last_checked_at.is_none() && show.folder_path.is_none());

    // A feed is in a library once; another podcasts library may hold it.
    let again = sqlx::query(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title)
         VALUES ($1, $2, 'Dup', 'Dup')",
    )
    .bind(pods)
    .bind(feed)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(code(again), UNIQUE_VIOLATION);
    let (elsewhere,): (Uuid,) = sqlx::query_as(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title)
         VALUES ($1, $2, 'The Show', 'Show') RETURNING id",
    )
    .bind(pods_two)
    .bind(feed)
    .fetch_one(&pool)
    .await
    .expect("the same feed in another library");
    // Out-of-range settings are refused.
    for (column, sql) in [
        (
            "max_episodes_to_keep",
            "UPDATE podcasts SET max_episodes_to_keep = -1 WHERE id = $1",
        ),
        (
            "max_new_episodes_per_check",
            "UPDATE podcasts SET max_new_episodes_per_check = -1 WHERE id = $1",
        ),
        (
            "consecutive_failures",
            "UPDATE podcasts SET consecutive_failures = -1 WHERE id = $1",
        ),
        (
            "podcast_type",
            "UPDATE podcasts SET podcast_type = 'weekly' WHERE id = $1",
        ),
    ] {
        let e = sqlx::query(sql)
            .bind(show.id)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(code(e), CHECK_VIOLATION, "{column}");
    }
    // The refresh state POD2 keeps: validators, last check, failures in a
    // row, and auto-download turning itself off after the 24th.
    sqlx::query(
        "UPDATE podcasts SET etag = '\"abc\"', last_modified = 'Wed, 25 Nov 2026 10:00:00 GMT',
                last_checked_at = now(), last_error = 'HTTP 503', consecutive_failures = 24,
                auto_download = false, podcast_type = 'serial', genres = '{Technology}'
         WHERE id = $1",
    )
    .bind(show.id)
    .execute(&pool)
    .await
    .expect("refresh state");

    // --------------------------------------------------------- episodes
    let published = |day: u32| Utc.with_ymd_and_hms(2026, 11, day, 6, 0, 0).unwrap();
    let insert_episode = |guid: &'static str, day: u32, file: Option<&'static str>| {
        let pool = pool.clone();
        let podcast = show.id;
        async move {
            sqlx::query_as::<_, (Uuid,)>(
                "INSERT INTO podcast_episodes
                     (podcast_id, guid, title, published_at, duration_ms, episode_type,
                      enclosure_url, enclosure_type, enclosure_length,
                      file_path, file_size_bytes, file_duration_ms, file_container, file_codec,
                      downloaded_at)
                 VALUES ($1, $2::text, 'Episode ' || $2::text, $3, 600000, 'full',
                         'https://cdn.example.test/' || $2::text || '.mp3', 'audio/mpeg',
                         9600000, $4::text,
                         CASE WHEN $4::text IS NOT NULL THEN 9600000 END,
                         CASE WHEN $4::text IS NOT NULL THEN 600000 END,
                         CASE WHEN $4::text IS NOT NULL THEN 'mp3' END,
                         CASE WHEN $4::text IS NOT NULL THEN 'mp3' END,
                         CASE WHEN $4::text IS NOT NULL THEN now() END)
                 RETURNING id",
            )
            .bind(podcast)
            .bind(guid)
            .bind(published(day))
            .bind(file)
            .fetch_one(&pool)
            .await
            .map(|(id,)| id)
        }
    };
    let downloaded = insert_episode("ep-1", 20, Some("/podcasts/The Show/ep-1.mp3"))
        .await
        .expect("a downloaded episode");
    let listed = insert_episode("ep-2", 25, None)
        .await
        .expect("an episode the feed lists, not downloaded");
    // The guid is unique within its show (and only there).
    let e = insert_episode("ep-1", 26, None).await.unwrap_err();
    assert_eq!(code(e), UNIQUE_VIOLATION);
    sqlx::query(
        "INSERT INTO podcast_episodes (podcast_id, guid, title, enclosure_url)
         VALUES ($1, 'ep-1', 'Same guid, other show', 'https://cdn.example.test/x.mp3')",
    )
    .bind(elsewhere)
    .execute(&pool)
    .await
    .expect("the same guid in another show");
    // A local file and its download time come together.
    let e = sqlx::query("UPDATE podcast_episodes SET downloaded_at = now() WHERE id = $1")
        .bind(listed)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(code(e), CHECK_VIOLATION);
    // Two episodes can't claim one file.
    let e = sqlx::query(
        "UPDATE podcast_episodes SET file_path = '/podcasts/The Show/ep-1.mp3',
                downloaded_at = now() WHERE id = $1",
    )
    .bind(listed)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(code(e), UNIQUE_VIOLATION);
    let e = sqlx::query("UPDATE podcast_episodes SET episode_type = 'teaser' WHERE id = $1")
        .bind(listed)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(code(e), CHECK_VIOLATION);
    // An item without an enclosure isn't an episode.
    let e = sqlx::query(
        "INSERT INTO podcast_episodes (podcast_id, guid, title) VALUES ($1, 'ep-x', 'No audio')",
    )
    .bind(show.id)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(code(e), "23502", "enclosure_url is NOT NULL");

    // ------------------------------------------ access through the grant (D31)
    assert_eq!(visible_episodes(&pool, ada).await, ["ep-2", "ep-1"]);
    assert!(
        visible_episodes(&pool, kid).await.is_empty(),
        "a profile without a grant on the podcasts library sees no episode"
    );
    app.grant(pods, kid).await;
    assert_eq!(
        visible_episodes(&pool, kid).await,
        ["ep-2", "ep-1"],
        "with the grant it sees every show in the library — nothing to subscribe to"
    );
    sqlx::query("DELETE FROM library_access WHERE profile_id = $1 AND library_id = $2")
        .bind(kid)
        .bind(pods)
        .execute(&pool)
        .await
        .unwrap();
    assert!(visible_episodes(&pool, kid).await.is_empty());

    // ------------------------------- episode progress, AB11's rules (D29)
    let sent = Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, 0).unwrap();
    let p = write_episode(
        &pool,
        ada,
        downloaded,
        ProgressWrite {
            position_ms: Some(120_000),
            ..Default::default()
        },
        Some(sent),
    )
    .await;
    assert_eq!(p.item_id, None, "an episode's row names no book");
    assert_eq!(p.position_ms, 120_000);
    assert_eq!(p.duration_ms, Some(600_000), "the downloaded file's length");
    assert!(!p.finished && p.finished_at.is_none() && !p.hidden);
    assert!(p.last_listened_at.is_some());
    assert_eq!(
        p.client_updated_at,
        Some(sent),
        "the client's own time, stored"
    );
    // Clamped to the episode; a position inside the last 10 s finishes it
    // (the podcasts library's own auto-finish rule, ABS's default).
    let p = write_episode(
        &pool,
        ada,
        downloaded,
        ProgressWrite {
            position_ms: Some(595_000),
            ..Default::default()
        },
        None,
    )
    .await;
    assert!(p.finished && p.finished_at.is_some(), "{p:?}");
    assert!(
        p.client_updated_at.is_some(),
        "the server's time when none was sent"
    );
    let p = write_episode(
        &pool,
        ada,
        downloaded,
        ProgressWrite {
            position_ms: Some(9_000_000),
            ..Default::default()
        },
        None,
    )
    .await;
    assert_eq!(p.position_ms, 600_000, "clamped to the episode's length");
    // Hidden from Continue Listening, then cleared by the next move.
    let p = write_episode(
        &pool,
        ada,
        downloaded,
        ProgressWrite {
            hidden: Some(true),
            ..Default::default()
        },
        None,
    )
    .await;
    assert!(p.hidden && p.finished);
    // Un-finishing by hand puts the position back to 0.
    let p = write_episode(
        &pool,
        ada,
        downloaded,
        ProgressWrite {
            finished: Some(false),
            ..Default::default()
        },
        None,
    )
    .await;
    assert!(!p.finished && p.finished_at.is_none());
    assert_eq!(p.position_ms, 0);
    assert!(!p.hidden, "a change clears the hide flag");
    // Per profile per episode: the kid's place is its own row.
    write_episode(
        &pool,
        kid,
        downloaded,
        ProgressWrite {
            position_ms: Some(30_000),
            ..Default::default()
        },
        None,
    )
    .await;
    let rows = count(
        &pool,
        "SELECT count(*) FROM book_progress WHERE episode_id = $1",
        downloaded,
    )
    .await;
    assert_eq!(rows, 2, "one row per profile, every write an upsert");

    // A row is a book's or an episode's, never both and never neither,
    // and each is unique per profile.
    let (book,): (Uuid,) = sqlx::query_as(
        "INSERT INTO media_items (library_id, kind, stable_key, title, sort_title)
         VALUES ($1, 'audiobook', 'audiobook:Jane Doe/A Book', 'A Book', 'Book')
         RETURNING id",
    )
    .bind(books)
    .fetch_one(&pool)
    .await
    .unwrap();
    let e = sqlx::query(
        "INSERT INTO book_progress (profile_id, item_id, episode_id) VALUES ($1, $2, $3)",
    )
    .bind(ada)
    .bind(book)
    .bind(listed)
    .execute(&pool)
    .await
    .unwrap_err();
    assert_eq!(code(e), CHECK_VIOLATION, "both a book and an episode");
    let e = sqlx::query("INSERT INTO book_progress (profile_id) VALUES ($1)")
        .bind(ada)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(code(e), CHECK_VIOLATION, "neither");
    let e = sqlx::query("INSERT INTO book_progress (profile_id, episode_id) VALUES ($1, $2)")
        .bind(ada)
        .bind(downloaded)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(code(e), UNIQUE_VIOLATION, "one row per profile and episode");

    // ------------------------ the book side is unchanged by the shared table
    // AB4's upsert still finds its (profile, item) key.
    for position in [1_000_i64, 2_000] {
        sqlx::query(
            "INSERT INTO book_progress (profile_id, item_id, position_ms) VALUES ($1, $2, $3)
             ON CONFLICT (profile_id, item_id) DO UPDATE SET position_ms = $3",
        )
        .bind(ada)
        .bind(book)
        .bind(position)
        .execute(&pool)
        .await
        .expect("book upsert");
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM book_progress WHERE item_id = $1",
            book
        )
        .await,
        1
    );
    // The bulk book sync lists the book and never an episode's row.
    let (status, synced) = app
        .req(Method::GET, "/api/v1/audiobooks/progress", None)
        .await;
    assert_eq!(status, 200, "{synced}");
    let synced = synced["progress"].as_array().unwrap().clone();
    assert_eq!(synced.len(), 1, "{synced:?}");
    assert_eq!(synced[0]["item_id"], json!(book));
    assert_eq!(synced[0]["position_ms"], 2_000);

    // --------------------------------------------------------- cascades
    write_episode(
        &pool,
        ada,
        listed,
        ProgressWrite {
            finished: Some(true),
            ..Default::default()
        },
        None,
    )
    .await;
    sqlx::query("DELETE FROM podcast_episodes WHERE id = $1")
        .bind(listed)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM book_progress WHERE episode_id = $1",
            listed
        )
        .await,
        0,
        "an episode's progress goes with it"
    );
    let (status, _) = app
        .req(Method::DELETE, &format!("/api/v1/profiles/{kid}"), None)
        .await;
    assert_eq!(status, 204);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM book_progress WHERE episode_id = $1",
            downloaded
        )
        .await,
        1,
        "a deleted profile's progress goes with it; the other stays"
    );
    sqlx::query("DELETE FROM libraries WHERE id = $1")
        .bind(pods)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM podcasts WHERE library_id = $1",
            pods
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM book_progress WHERE profile_id = $1 AND episode_id IS NOT NULL",
            ada
        )
        .await,
        0,
        "removing the library removes its shows, episodes and their progress"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM book_progress WHERE profile_id = $1",
            ada
        )
        .await,
        1,
        "the book's progress stays"
    );
    println!("POD1 schema flow: ok");
}
