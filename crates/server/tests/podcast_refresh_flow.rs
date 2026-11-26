//! End-to-end test of POD2 (feed fetch + parse, D32) against a real
//! Postgres and a local stub feed server:
//!
//! - the scheduled check runs only while a show's `auto_download` is
//!   on, at its cron schedule; the tick queues one `refresh_podcasts`
//!   job and the job checks the shows that are due;
//! - a 200 stores the show as the feed describes it, its validators,
//!   and a row per feed episode (file columns empty; nothing is
//!   downloaded) — the fixtures are the parser's own;
//! - the conditional GET sends `If-None-Match` / `If-Modified-Since`,
//!   and a 304 leaves the show alone but stamps the check;
//! - a changed feed adds new episodes, updates changed ones, matches
//!   a re-issued guid by its enclosure URL and leaves episodes gone
//!   from the feed alone;
//! - every failure is stamped and counted; the 24th in a row turns
//!   `auto_download` off; a success clears the error and the count;
//! - the Dashboard notice appears from the 3rd failure in a row (with
//!   the count), turns to an error at the 24th, and clears after a
//!   success;
//! - a job naming one show checks it whatever its schedule.
//!
//! POD2 builds no API: the shows are written with SQL, as POD4 will
//! write them. Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a
//! fresh database).
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_refresh_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use parlour_catalog::podcasts::{check_podcast, due_podcasts, CheckOutcome, MAX_FAILED_CHECKS};
use parlour_server::config::Config;
use parlour_server::http::AppState;
use parlour_server::notices;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const EPISODIC: &str = include_str!("../../catalog/tests/fixtures/podcasts/episodic.xml");
const SERIAL: &str = include_str!("../../catalog/tests/fixtures/podcasts/serial.xml");
const NOT_A_FEED: &str = include_str!("../../catalog/tests/fixtures/podcasts/not-a-feed.html");

// ------------------------------------------------------------ stub feeds

#[derive(Clone)]
struct Served {
    body: String,
    etag: Option<String>,
    last_modified: Option<String>,
    /// Answer with this status instead (a failing host).
    status: Option<u16>,
}

/// (feed, If-None-Match, If-Modified-Since) of one request.
type Seen = (String, Option<String>, Option<String>);

#[derive(Clone, Default)]
struct Stub {
    feeds: Arc<Mutex<HashMap<String, Served>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn serve(&self, name: &str, served: Served) {
        self.feeds.lock().unwrap().insert(name.to_string(), served);
    }

    fn last_request(&self, name: &str) -> (Option<String>, Option<String>) {
        let seen = self.seen.lock().unwrap();
        let (_, inm, ims) = seen
            .iter()
            .rev()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("no request for {name}"));
        (inm.clone(), ims.clone())
    }

    fn requests(&self, name: &str) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _, _)| n == name)
            .count()
    }
}

async fn stub_feed(
    State(stub): State<Stub>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let header = |h: &str| {
        headers
            .get(h)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let inm = header("if-none-match");
    let ims = header("if-modified-since");
    stub.seen
        .lock()
        .unwrap()
        .push((name.clone(), inm.clone(), ims.clone()));
    let Some(feed) = stub.feeds.lock().unwrap().get(&name).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(status) = feed.status {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let fresh = match (&feed.etag, &feed.last_modified) {
        (Some(etag), _) => inm.as_ref() == Some(etag),
        (None, Some(lm)) => ims.as_ref() == Some(lm),
        (None, None) => false,
    };
    if fresh {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mut out = HeaderMap::new();
    out.insert("content-type", "application/rss+xml".parse().unwrap());
    if let Some(etag) = &feed.etag {
        out.insert("etag", etag.parse().unwrap());
    }
    if let Some(lm) = &feed.last_modified {
        out.insert("last-modified", lm.parse().unwrap());
    }
    (StatusCode::OK, out, feed.body).into_response()
}

fn ok(body: &str, etag: Option<&str>, last_modified: Option<&str>) -> Served {
    Served {
        body: body.to_string(),
        etag: etag.map(str::to_string),
        last_modified: last_modified.map(str::to_string),
        status: None,
    }
}

fn failing(status: u16) -> Served {
    Served {
        body: String::new(),
        etag: None,
        last_modified: None,
        status: Some(status),
    }
}

// --------------------------------------------------------------- client

fn test_config(url: &str) -> Config {
    let mut vars = HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), url.to_string());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    Config::from_vars(&vars).expect("config")
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
            .header("Cookie", format!("parlour_session={}", self.cookie));
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let resp = builder.send().await.expect("request");
        let status = resp.status().as_u16();
        let value = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }

    async fn feed_notices(&self) -> Vec<Value> {
        let (status, list) = self.req(Method::GET, "/api/v1/admin/notices", None).await;
        assert_eq!(status, 200, "{list}");
        list.as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "podcast_feed_failing")
            .cloned()
            .collect()
    }
}

async fn add_show(pool: &PgPool, library: Uuid, feed_url: &str, title: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title)
         VALUES ($1, $2, $3, lower($3)) RETURNING id",
    )
    .bind(library)
    .bind(feed_url)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

#[derive(Debug, sqlx::FromRow)]
struct ShowRow {
    title: String,
    sort_title: String,
    author: Option<String>,
    description: Option<String>,
    language: Option<String>,
    explicit: bool,
    podcast_type: Option<String>,
    genres: Vec<String>,
    image_url: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    last_checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    consecutive_failures: i32,
    auto_download: bool,
    check_schedule: String,
    updated_at: DateTime<Utc>,
}

async fn show(pool: &PgPool, id: Uuid) -> ShowRow {
    sqlx::query_as(
        "SELECT title, sort_title, author, description, language, explicit, podcast_type,
                genres, image_url, etag, last_modified, last_checked_at, last_error,
                consecutive_failures, auto_download, check_schedule, updated_at
         FROM podcasts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct EpisodeRow {
    guid: String,
    title: String,
    published_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    season: Option<i32>,
    episode_number: Option<i32>,
    episode_type: Option<String>,
    enclosure_url: String,
    enclosure_length: Option<i64>,
    chapters_url: Option<String>,
    transcript_url: Option<String>,
    transcript_type: Option<String>,
    file_path: Option<String>,
    downloaded_at: Option<DateTime<Utc>>,
}

async fn episodes(pool: &PgPool, podcast: Uuid) -> Vec<EpisodeRow> {
    sqlx::query_as(
        "SELECT guid, title, published_at, duration_ms, season, episode_number, episode_type,
                enclosure_url, enclosure_length, chapters_url, transcript_url,
                transcript_type, file_path, downloaded_at
         FROM podcast_episodes WHERE podcast_id = $1
         ORDER BY published_at DESC NULLS LAST, guid",
    )
    .bind(podcast)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn run_queued_job(pool: &PgPool, http: &reqwest::Client) -> parlour_jobs::Job {
    let job = parlour_jobs::claim_next(pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .expect("a queued refresh_podcasts job");
    let id = job.id;
    parlour_server::podcasts::run_refresh_job_claimed(pool, http, job).await;
    parlour_jobs::get(pool, id).await.unwrap().unwrap()
}

async fn check(pool: &PgPool, http: &reqwest::Client, id: Uuid) -> CheckOutcome {
    check_podcast(pool, http, id)
        .await
        .unwrap()
        .expect("the show exists")
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn podcast_refresh_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    // The app, for the notices endpoint.
    let state = AppState::new(pool.clone(), None, false);
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
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .and_then(|kv| kv.split_once('='))
        .map(|(_, v)| v.to_string())
        .expect("session cookie");
    let app = TestApp {
        base,
        client,
        cookie,
    };

    // The stub feed host.
    let stub = Stub::default();
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let feeds = format!("http://{}/feeds", stub_listener.local_addr().unwrap());
    {
        let app_stub = Router::new()
            .route("/feeds/{name}", get(stub_feed))
            .with_state(stub.clone());
        tokio::spawn(async move {
            axum::serve(stub_listener, app_stub).await.unwrap();
        });
    }
    let http = parlour_catalog::podcasts::http_client();

    // A podcasts library and four shows, added as POD4 will add them.
    let (status, lib) = app
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Podcasts", "kind": "podcasts", "paths": []})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library = Uuid::parse_str(lib["id"].as_str().unwrap()).unwrap();
    let lantern = add_show(&pool, library, &format!("{feeds}/lantern"), "Pending A").await;
    let ice = add_show(&pool, library, &format!("{feeds}/ice"), "Pending B").await;
    let broken = add_show(&pool, library, &format!("{feeds}/broken"), "Broken Show").await;
    let page = add_show(&pool, library, &format!("{feeds}/page"), "Moved Show").await;
    stub.serve("lantern", ok(EPISODIC, Some("\"lantern-v1\""), None));
    stub.serve(
        "ice",
        ok(SERIAL, None, Some("Sat, 07 Nov 2026 05:00:00 GMT")),
    );
    stub.serve("broken", failing(503));
    stub.serve(
        "page",
        ok(
            NOT_A_FEED,
            Some("\"page-v1\""),
            Some("Sun, 01 Nov 2026 00:00:00 GMT"),
        ),
    );

    // ---------------------------------------------- 1. the schedule switch
    // auto_download is off by default (D32), and it is the schedule
    // switch (POD1 reading 5): nothing is due and nothing is queued.
    let now = Utc::now();
    assert!(due_podcasts(&pool, now).await.unwrap().is_empty());
    assert_eq!(
        parlour_server::podcasts::tick(&pool, now).await.unwrap(),
        None
    );
    sqlx::query("UPDATE podcasts SET auto_download = true WHERE id = ANY($1)")
        .bind(vec![lantern, ice, broken])
        .execute(&pool)
        .await
        .unwrap();
    let due: Vec<Uuid> = due_podcasts(&pool, now)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(due.len(), 3, "never checked: due at once");
    assert!(!due.contains(&page), "auto_download off: not scheduled");
    let job_id = parlour_server::podcasts::tick(&pool, now)
        .await
        .unwrap()
        .expect("a job is queued");
    assert_eq!(
        parlour_server::podcasts::tick(&pool, now).await.unwrap(),
        None,
        "one waiting job is enough"
    );

    // ------------------------------------------------ 2. the first check
    let job = run_queued_job(&pool, &http).await;
    assert_eq!(job.id, job_id);
    assert_eq!(
        job.status, "succeeded",
        "a feed's failure doesn't fail the job"
    );
    assert_eq!(job.payload["trigger"], "schedule");
    let result = job.result.clone().unwrap();
    assert_eq!(result["checked"], 3, "{result}");
    assert_eq!(result["updated"], 2, "{result}");
    assert_eq!(result["failed"], 1, "{result}");
    assert_eq!(result["new_episodes"], 6, "{result}");
    assert_eq!((job.progress_done, job.progress_total), (3, 3));
    let errors = job.errors.as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["path"], "Broken Show", "listed by title, not URL");
    assert_eq!(errors[0]["message"], "the feed answered HTTP 503");
    assert_eq!(stub.requests("page"), 0, "not scheduled, not fetched");

    // The show, as its feed describes it.
    let a = show(&pool, lantern).await;
    assert_eq!(a.title, "The Lantern Hour");
    assert_eq!(
        a.sort_title,
        parlour_catalog::parse::sort_title("The Lantern Hour")
    );
    assert_eq!(a.author.as_deref(), Some("Imaginary Radio"));
    assert!(a.description.unwrap().starts_with("Weekly conversations"));
    assert_eq!(a.language.as_deref(), Some("en-ca"));
    assert!(!a.explicit);
    assert_eq!(a.podcast_type.as_deref(), Some("episodic"));
    assert_eq!(
        a.genres,
        ["History", "Society & Culture", "Places & Travel"]
    );
    assert_eq!(
        a.image_url.as_deref(),
        Some("https://cdn.example.com/lantern-hour/cover-3000.jpg")
    );
    assert_eq!(a.etag.as_deref(), Some("\"lantern-v1\""));
    assert_eq!(a.last_modified, None);
    assert!(a.last_checked_at.is_some());
    assert_eq!((a.last_error, a.consecutive_failures), (None, 0));
    assert_eq!(
        stub.last_request("lantern"),
        (None, None),
        "nothing stored yet"
    );

    // A row per feed episode; nothing downloaded.
    let eps = episodes(&pool, lantern).await;
    assert_eq!(eps.len(), 3);
    let ep3 = &eps[0];
    assert_eq!(ep3.guid, "lantern-hour-0003");
    assert_eq!(ep3.title, "Ep. 3: The Keeper's Logbook");
    assert_eq!(ep3.duration_ms, Some(3_723_000));
    assert_eq!((ep3.season, ep3.episode_number), (Some(1), Some(3)));
    assert_eq!(ep3.episode_type.as_deref(), Some("full"));
    assert_eq!(
        ep3.enclosure_url,
        "https://media.example.com/lantern-hour/ep3.mp3?source=feed&n=3"
    );
    assert_eq!(ep3.enclosure_length, Some(48_213_760));
    assert!(ep3
        .chapters_url
        .as_deref()
        .unwrap()
        .ends_with("ep3.chapters.json"));
    assert!(ep3.transcript_url.as_deref().unwrap().ends_with("ep3.vtt"));
    assert_eq!(ep3.transcript_type.as_deref(), Some("text/vtt"));
    assert_eq!(eps[2].episode_type.as_deref(), Some("trailer"));
    assert!(eps
        .iter()
        .all(|e| e.file_path.is_none() && e.downloaded_at.is_none()));

    let b = show(&pool, ice).await;
    assert_eq!(b.podcast_type.as_deref(), Some("serial"));
    assert!(b.explicit);
    assert_eq!(b.etag, None);
    assert_eq!(
        b.last_modified.as_deref(),
        Some("Sat, 07 Nov 2026 05:00:00 GMT")
    );
    let ice_eps = episodes(&pool, ice).await;
    assert_eq!(ice_eps.len(), 3);
    let c2 = ice_eps.iter().find(|e| e.guid == "ice-shelf-c2").unwrap();
    assert_eq!(
        (c2.season, c2.episode_number),
        (None, None),
        "non-numeric dropped"
    );
    assert_eq!(c2.enclosure_length, None);

    let c = show(&pool, broken).await;
    assert!(
        c.last_checked_at.is_some(),
        "a failure stamps the check too"
    );
    assert_eq!(c.last_error.as_deref(), Some("the feed answered HTTP 503"));
    assert_eq!(c.consecutive_failures, 1);
    assert!(c.auto_download);

    // --------------------------------------------- 3. the conditional GET
    let checked_before = show(&pool, lantern).await.last_checked_at.unwrap();
    let updated_before = show(&pool, lantern).await.updated_at;
    assert_eq!(
        check(&pool, &http, lantern).await,
        CheckOutcome::NotModified
    );
    assert_eq!(
        stub.last_request("lantern"),
        (Some("\"lantern-v1\"".into()), None),
        "If-None-Match carries the stored ETag"
    );
    let a = show(&pool, lantern).await;
    assert!(
        a.last_checked_at.unwrap() > checked_before,
        "a 304 stamps the check"
    );
    assert_eq!(a.updated_at, updated_before, "a 304 changes nothing else");
    assert_eq!(a.etag.as_deref(), Some("\"lantern-v1\""));
    assert_eq!(episodes(&pool, lantern).await.len(), 3);
    assert_eq!(check(&pool, &http, ice).await, CheckOutcome::NotModified);
    assert_eq!(
        stub.last_request("ice"),
        (None, Some("Sat, 07 Nov 2026 05:00:00 GMT".into())),
        "If-Modified-Since carries the stored Last-Modified"
    );
    // A 200 whose content didn't change leaves updated_at alone too.
    stub.serve("lantern", ok(EPISODIC, Some("\"lantern-v1b\""), None));
    assert_eq!(
        check(&pool, &http, lantern).await,
        CheckOutcome::Updated {
            new_episodes: 0,
            changed_episodes: 0
        }
    );
    let a = show(&pool, lantern).await;
    assert_eq!(a.etag.as_deref(), Some("\"lantern-v1b\""));
    assert_eq!(a.updated_at, updated_before);

    // -------------------------------------------------- 4. due again later
    let now = Utc::now();
    assert!(
        due_podcasts(&pool, now).await.unwrap().is_empty(),
        "all checked this hour"
    );
    assert_eq!(
        due_podcasts(&pool, now + Duration::hours(1))
            .await
            .unwrap()
            .len(),
        3
    );
    // An unreadable schedule is read as the hourly default.
    sqlx::query("UPDATE podcasts SET check_schedule = 'whenever' WHERE id = $1")
        .bind(lantern)
        .execute(&pool)
        .await
        .unwrap();
    assert!(due_podcasts(&pool, now).await.unwrap().is_empty());
    assert_eq!(
        due_podcasts(&pool, now + Duration::hours(1))
            .await
            .unwrap()
            .len(),
        3
    );
    // A weekly schedule (Mondays 06:00) isn't due an hour on.
    sqlx::query("UPDATE podcasts SET check_schedule = '0 6 * * 1' WHERE id = $1")
        .bind(lantern)
        .execute(&pool)
        .await
        .unwrap();
    let in_an_hour: Vec<Uuid> = due_podcasts(&pool, now + Duration::hours(1))
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!in_an_hour.contains(&lantern));
    assert_eq!(
        due_podcasts(&pool, now + Duration::days(8))
            .await
            .unwrap()
            .len(),
        3
    );
    sqlx::query("UPDATE podcasts SET check_schedule = '0 * * * *' WHERE id = $1")
        .bind(lantern)
        .execute(&pool)
        .await
        .unwrap();

    // ------------------------------------------------- 5. a changed feed
    // A new episode at the top; episode 2 retitled; episode 3's guid
    // re-issued (same audio); the trailer gone from the feed.
    let new_item = r#"<item>
      <title>Ep. 4: Fog Signals</title>
      <guid isPermaLink="false">lantern-hour-0004</guid>
      <pubDate>Thu, 26 Nov 2026 10:00:00 +0000</pubDate>
      <enclosure url="https://media.example.com/lantern-hour/ep4.mp3" length="40000000" type="audio/mpeg"/>
      <itunes:duration>3000</itunes:duration>
      <itunes:season>1</itunes:season>
      <itunes:episode>4</itunes:episode>
    </item>
    <item>
      <title>Ep. 3: The Keeper's Logbook</title>"#;
    let trailer_start = EPISODIC.find("<item>\n      <title>Coming soon").unwrap();
    let trailer_end = EPISODIC[trailer_start..].find("</item>").unwrap() + trailer_start + 7;
    let changed = format!("{}{}", &EPISODIC[..trailer_start], &EPISODIC[trailer_end..])
        .replacen(
            "<item>\n      <title>Ep. 3: The Keeper's Logbook</title>",
            new_item,
            1,
        )
        .replace(
            "lantern-hour-0003",
            "https://lantern-hour.example.com/episodes/3",
        )
        .replace(
            "Ep. 2: Fresnel &amp; Friends",
            "Ep. 2: Fresnel, Lenses &amp; Friends",
        );
    stub.serve("lantern", ok(&changed, Some("\"lantern-v2\""), None));
    assert_eq!(
        check(&pool, &http, lantern).await,
        CheckOutcome::Updated {
            new_episodes: 1,
            changed_episodes: 1
        }
    );
    let eps = episodes(&pool, lantern).await;
    let guids: Vec<&str> = eps.iter().map(|e| e.guid.as_str()).collect();
    assert_eq!(
        guids,
        [
            "lantern-hour-0004",
            // Matched by its enclosure URL: the stored guid is kept.
            "lantern-hour-0003",
            "https://lantern-hour.example.com/episodes/2",
            // Gone from the feed: left alone (POD3 owns retention).
            "lantern-hour-trailer",
        ]
    );
    assert_eq!(
        eps[0].published_at.unwrap().to_rfc3339(),
        "2026-11-26T10:00:00+00:00"
    );
    assert_eq!(eps[2].title, "Ep. 2: Fresnel, Lenses & Friends");
    let a = show(&pool, lantern).await;
    assert_eq!(a.etag.as_deref(), Some("\"lantern-v2\""));

    // ----------------------------------------- 6. failures and the notice
    // Two failures: below the notice's threshold.
    assert!(app.feed_notices().await.is_empty());
    let outcome = check(&pool, &http, broken).await;
    assert_eq!(
        outcome,
        CheckOutcome::Failed {
            error: "the feed answered HTTP 503".into(),
            consecutive_failures: 2,
            schedule_turned_off: false,
        }
    );
    notices::reconcile_with(&pool, &[]).await.unwrap();
    assert!(
        app.feed_notices().await.is_empty(),
        "two is not yet repeatedly"
    );
    // A 200 that isn't a feed fails, and its validators are not kept.
    sqlx::query("UPDATE podcasts SET consecutive_failures = 1 WHERE id = $1")
        .bind(page)
        .execute(&pool)
        .await
        .unwrap();
    match check(&pool, &http, page).await {
        CheckOutcome::Failed {
            error,
            consecutive_failures,
            ..
        } => {
            assert_eq!(error, "not an RSS feed (the document's root is <html>)");
            assert_eq!(consecutive_failures, 2);
        }
        other => panic!("{other:?}"),
    }
    let p = show(&pool, page).await;
    assert_eq!((p.etag, p.last_modified), (None, None));
    assert_eq!(p.title, "Moved Show");

    // The third failure in a row raises the notice, with the count.
    stub.serve("broken", failing(500));
    check(&pool, &http, broken).await;
    notices::reconcile_with(&pool, &[]).await.unwrap();
    let list = app.feed_notices().await;
    assert_eq!(list.len(), 1, "{list:?}");
    let n = &list[0];
    assert_eq!(n["severity"], "warning");
    assert_eq!(
        n["summary"],
        "The feed for Broken Show has failed 3 checks in a row"
    );
    assert_eq!(n["detail"]["failures"], 3);
    assert_eq!(n["detail"]["error"], "the feed answered HTTP 500");
    assert_eq!(n["detail"]["podcast_id"], broken.to_string());
    assert_eq!(n["detail"]["library"], "Podcasts");
    assert!(
        !n.to_string().contains(&feeds),
        "the feed URL stays out of the notice"
    );
    let notice_id = n["id"].as_str().unwrap().to_string();

    // A 404 is a failure like any other.
    stub.feeds.lock().unwrap().remove("broken");
    assert!(matches!(
        check(&pool, &http, broken).await,
        CheckOutcome::Failed { ref error, consecutive_failures: 4, .. }
            if error == "the feed answered HTTP 404"
    ));
    notices::reconcile_with(&pool, &[]).await.unwrap();
    let list = app.feed_notices().await;
    assert_eq!(
        list[0]["id"],
        notice_id.as_str(),
        "the same notice, updated"
    );
    assert_eq!(
        list[0]["summary"],
        "The feed for Broken Show has failed 4 checks in a row"
    );

    // Up to the 23rd: still scheduled.
    sqlx::query("UPDATE podcasts SET consecutive_failures = $2 WHERE id = $1")
        .bind(broken)
        .bind(MAX_FAILED_CHECKS - 2)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        check(&pool, &http, broken).await,
        CheckOutcome::Failed {
            consecutive_failures: 23,
            schedule_turned_off: false,
            ..
        }
    ));
    assert!(show(&pool, broken).await.auto_download);
    // The 24th turns auto_download off; the schedule itself is kept.
    let updated_before = show(&pool, broken).await.updated_at;
    assert!(matches!(
        check(&pool, &http, broken).await,
        CheckOutcome::Failed {
            consecutive_failures: 24,
            schedule_turned_off: true,
            ..
        }
    ));
    let c = show(&pool, broken).await;
    assert!(!c.auto_download);
    assert_eq!(c.check_schedule, "0 * * * *");
    assert!(c.updated_at > updated_before, "a settings change");
    let due: Vec<Uuid> = due_podcasts(&pool, Utc::now() + Duration::hours(1))
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!due.contains(&broken), "no longer scheduled");
    // A 25th (a check by hand) counts on, and turns nothing off twice.
    assert!(matches!(
        check(&pool, &http, broken).await,
        CheckOutcome::Failed {
            consecutive_failures: 25,
            schedule_turned_off: false,
            ..
        }
    ));
    notices::reconcile_with(&pool, &[]).await.unwrap();
    let list = app.feed_notices().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["severity"], "error");
    assert_eq!(
        list[0]["summary"],
        "The feed for Broken Show has failed 25 checks in a row; its scheduled check is off"
    );
    assert_eq!(list[0]["detail"]["auto_download"], false);

    // ---------------------- 7. a job naming one show, and a success that clears
    stub.serve("broken", ok(SERIAL, Some("\"broken-back\""), None));
    parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::RefreshPodcasts,
        None,
        json!({ "trigger": "manual", "podcast_id": broken }),
    )
    .await
    .unwrap();
    let job = run_queued_job(&pool, &http).await;
    assert_eq!(job.status, "succeeded");
    let result = job.result.unwrap();
    assert_eq!(result["checked"], 1, "{result}");
    assert_eq!(result["updated"], 1, "{result}");
    let c = show(&pool, broken).await;
    assert_eq!((c.last_error, c.consecutive_failures), (None, 0));
    assert_eq!(c.title, "Under the Ice Shelf");
    assert!(
        !c.auto_download,
        "a success doesn't turn the schedule back on"
    );
    notices::reconcile_with(&pool, &[]).await.unwrap();
    assert!(app.feed_notices().await.is_empty(), "the notice clears");
    let (resolved,): (Option<DateTime<Utc>>,) =
        sqlx::query_as("SELECT resolved_at FROM notices WHERE key = $1")
            .bind(format!("podcast_feed_failing:{broken}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(resolved.is_some());

    // The page show still fails, below the threshold: no notice for it.
    assert_eq!(show(&pool, page).await.consecutive_failures, 2);

    // A job naming a show that isn't there fails nothing.
    parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::RefreshPodcasts,
        None,
        json!({ "trigger": "manual", "podcast_id": Uuid::new_v4() }),
    )
    .await
    .unwrap();
    let job = run_queued_job(&pool, &http).await;
    assert_eq!(job.status, "succeeded");
    assert_eq!(job.result.unwrap()["checked"], 0);

    // No refresh job failed along the way: feed trouble stays on the shows.
    let (failed_jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs WHERE kind = 'refresh_podcasts' AND status = 'failed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed_jobs, 0);
}
