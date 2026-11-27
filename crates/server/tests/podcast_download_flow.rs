//! End-to-end test of POD3 (episode download + retention, D32) against
//! a real Postgres, a local stub feed + enclosure host, real ffprobe,
//! and a throwaway folder standing in for the podcast store:
//!
//! - after a successful check of a show with `auto_download` on, the
//!   same `refresh_podcasts` run downloads the show's new episodes:
//!   newest first, at most `max_new_episodes_per_check`, into the
//!   show's folder (named from its title, unique in the store), with
//!   the `file_*` columns probed and `downloaded_at` set; no temporary
//!   file is left;
//! - "new" is as in Audiobookshelf: published after the newest
//!   downloaded episode, so a later check doesn't work through the back
//!   catalogue;
//! - retention deletes the oldest downloaded episode's file when the
//!   show holds more than `max_episodes_to_keep`, at most one per
//!   check; the row stays, "not downloaded", with its progress, is not
//!   downloaded again by a check, and can be downloaded by hand;
//! - a job naming one episode downloads it whatever its show's
//!   settings, and leaves one that already has a file alone;
//! - a failed download (an HTTP error, a file that isn't audio) leaves
//!   the row undownloaded and no file, is listed on the job by show
//!   and episode title, never by URL, and doesn't fail the job;
//! - a store that overlaps another library's folder, a podcasts
//!   library without a folder, and a store that can't be written all
//!   fail cleanly.
//!
//! POD3 builds no API: libraries and shows are written with SQL, as
//! POD4 will write them. Ignored by default; needs
//! PARLOUR_TEST_DATABASE_URL (a fresh database) and ffmpeg/ffprobe on
//! the PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_download_flow -- --ignored

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

// ------------------------------------------------------------ stub host

#[derive(Clone)]
struct Audio {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

#[derive(Clone, Default)]
struct Stub {
    feeds: Arc<Mutex<HashMap<String, String>>>,
    audio: Arc<Mutex<HashMap<String, Audio>>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl Stub {
    fn feed(&self, name: &str, body: String) {
        self.feeds.lock().unwrap().insert(name.into(), body);
    }
    fn audio(&self, name: &str, audio: Audio) {
        self.audio.lock().unwrap().insert(name.into(), audio);
    }
    fn hits(&self, name: &str) -> usize {
        *self.hits.lock().unwrap().get(name).unwrap_or(&0)
    }
}

async fn stub_feed(State(stub): State<Stub>, UrlPath(name): UrlPath<String>) -> Response {
    match stub.feeds.lock().unwrap().get(&name).cloned() {
        Some(body) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", "application/rss+xml".parse().unwrap());
            (StatusCode::OK, h, body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn stub_audio(State(stub): State<Stub>, UrlPath(name): UrlPath<String>) -> Response {
    *stub.hits.lock().unwrap().entry(name.clone()).or_default() += 1;
    match stub.audio.lock().unwrap().get(&name).cloned() {
        Some(a) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", a.content_type.parse().unwrap());
            (StatusCode::from_u16(a.status).unwrap(), h, a.body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mp3(body: &[u8]) -> Audio {
    Audio {
        status: 200,
        content_type: "audio/mpeg",
        body: body.to_vec(),
    }
}

/// One feed item: (guid, title, day in November 2026, enclosure URL).
fn feed(title: &str, items: &[(&str, &str, u32, String)]) -> String {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel><title>{title}</title><description>A test show</description>
"#
    );
    for (guid, ep_title, day, url) in items {
        xml.push_str(&format!(
            "<item><guid>{guid}</guid><title>{ep_title}</title>\
             <pubDate>{day:02} Nov 2026 07:00:00 +0000</pubDate>\
             <enclosure url=\"{}\" type=\"audio/mpeg\" length=\"1000\"/></item>\n",
            url.replace('&', "&amp;")
        ));
    }
    xml.push_str("</channel></rss>\n");
    xml
}

// ------------------------------------------------------------ fixtures

/// A few seconds of tone, made with ffmpeg.
fn tone(dir: &Path, name: &str, seconds: u32, args: &[&str]) -> Vec<u8> {
    let out = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=440:duration={seconds}"))
        .args(args)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg made {name}");
    std::fs::read(&out).unwrap()
}

async fn library(pool: &PgPool, name: &str, kind: &str, path: Option<&Path>) -> Uuid {
    let (id,): (Uuid,) =
        sqlx::query_as("INSERT INTO libraries (name, kind) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(kind)
            .fetch_one(pool)
            .await
            .unwrap();
    if let Some(path) = path {
        sqlx::query("INSERT INTO library_paths (library_id, path) VALUES ($1, $2)")
            .bind(id)
            .bind(path.to_string_lossy().as_ref())
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

/// A show as POD4 will add it, created before the fixtures' episodes
/// were published.
async fn add_show(pool: &PgPool, library: Uuid, feed_url: &str, title: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title, created_at)
         VALUES ($1, $2, $3, lower($3), '2026-10-01T00:00:00Z') RETURNING id",
    )
    .bind(library)
    .bind(feed_url)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Ep {
    id: Uuid,
    guid: String,
    file_path: Option<String>,
    file_size_bytes: Option<i64>,
    file_duration_ms: Option<i64>,
    file_container: Option<String>,
    file_codec: Option<String>,
    downloaded_at: Option<DateTime<Utc>>,
}

async fn episodes(pool: &PgPool, podcast: Uuid) -> HashMap<String, Ep> {
    let rows: Vec<Ep> = sqlx::query_as(
        "SELECT id, guid, file_path, file_size_bytes, file_duration_ms, file_container,
                file_codec, downloaded_at
         FROM podcast_episodes WHERE podcast_id = $1",
    )
    .bind(podcast)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter().map(|e| (e.guid.clone(), e)).collect()
}

fn downloaded(eps: &HashMap<String, Ep>) -> Vec<String> {
    let mut v: Vec<String> = eps
        .values()
        .filter(|e| e.file_path.is_some())
        .map(|e| e.guid.clone())
        .collect();
    v.sort();
    v
}

async fn folder_of(pool: &PgPool, podcast: Uuid) -> Option<String> {
    let (f,): (Option<String>,) = sqlx::query_as("SELECT folder_path FROM podcasts WHERE id = $1")
        .bind(podcast)
        .fetch_one(pool)
        .await
        .unwrap();
    f
}

/// The names in a folder, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

async fn run_job(pool: &PgPool, http: &reqwest::Client, payload: Value) -> parlour_jobs::Job {
    let id = parlour_jobs::enqueue(pool, parlour_jobs::JobKind::RefreshPodcasts, None, payload)
        .await
        .unwrap();
    let job = parlour_jobs::claim_next(pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .expect("the queued job");
    assert_eq!(job.id, id);
    parlour_server::podcasts::run_refresh_job_claimed(pool, http, job).await;
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    assert_eq!(
        job.status, "succeeded",
        "only a database error fails the job"
    );
    job
}

async fn check(pool: &PgPool, http: &reqwest::Client, show: Uuid) -> parlour_jobs::Job {
    run_job(pool, http, json!({ "podcast_id": show })).await
}

async fn download_by_hand(pool: &PgPool, http: &reqwest::Client, ep: Uuid) -> parlour_jobs::Job {
    run_job(pool, http, json!({ "episode_id": ep })).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn podcast_download_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    // The first admin and its profile, for a progress row.
    let mut vars = HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), url.clone());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    let cfg = parlour_server::config::Config::from_vars(&vars).expect("config");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    // The throwaway folders: a store, a films folder, fixtures.
    let root = std::env::temp_dir().join(format!("pod3-flow-{}", Uuid::new_v4().simple()));
    let store = root.join("podcasts");
    let films = root.join("films");
    let fixtures = root.join("fixtures");
    for d in [&store, &films, &fixtures] {
        std::fs::create_dir_all(d).unwrap();
    }
    let tone_mp3 = tone(
        &fixtures,
        "tone.mp3",
        3,
        &["-c:a", "libmp3lame", "-b:a", "64k"],
    );
    let tone_m4a = tone(&fixtures, "tone.m4a", 2, &["-c:a", "aac", "-b:a", "64k"]);

    // The stub host.
    let stub = Stub::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", listener.local_addr().unwrap());
    {
        let app = Router::new()
            .route("/feeds/{name}", get(stub_feed))
            .route("/audio/{name}", get(stub_audio))
            .with_state(stub.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    }
    // A private token, as a paid feed puts in its enclosure URLs (made
    // at run time). It must never reach a job's errors.
    let private = Uuid::new_v4().simple().to_string();
    let audio = |name: &str| format!("{host}/audio/{name}?token={private}&n=1");
    let http = parlour_catalog::podcasts::http_client();

    let pods = library(&pool, "Podcasts", "podcasts", Some(&store)).await;
    library(&pool, "Films", "movies", Some(&films)).await;

    // ------------------------------------------- 1. the first scheduled check
    // Lantern: auto-download on, 2 new per check, keep 2.
    let lantern = add_show(
        &pool,
        pods,
        &format!("{host}/feeds/lantern"),
        "Lantern / Hour: Live",
    )
    .await;
    sqlx::query(
        "UPDATE podcasts SET auto_download = true, max_new_episodes_per_check = 2,
                max_episodes_to_keep = 2
         WHERE id = $1",
    )
    .bind(lantern)
    .execute(&pool)
    .await
    .unwrap();
    for n in 1..=7 {
        stub.audio(&format!("l{n}.mp3"), mp3(&tone_mp3));
    }
    // l2 has no extension in its URL: the probe names it.
    stub.audio("l2", mp3(&tone_mp3));
    let mut items = vec![
        ("l1", "One", 1, audio("l1.mp3")),
        ("l2", "Two", 2, audio("l2")),
        ("l3", "Three", 3, audio("l3.mp3")),
        ("l4", "Four", 4, audio("l4.mp3")),
    ];
    stub.feed("lantern", feed("Lantern / Hour: Live", &items));

    // The tick queues the scheduled check; the run downloads after it.
    let job_id = parlour_server::podcasts::tick(&pool, Utc::now())
        .await
        .unwrap()
        .expect("lantern is due");
    let job = parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.id, job_id);
    parlour_server::podcasts::run_refresh_job_claimed(&pool, &http, job).await;
    let job = parlour_jobs::get(&pool, job_id).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded");
    let result = job.result.clone().unwrap();
    assert_eq!(result["new_episodes"], 4, "{result}");
    assert_eq!(result["downloaded"], 2, "max 2 new per check: {result}");
    assert_eq!(result["download_failed"], 0, "{result}");
    assert_eq!(result["removed"], 0, "{result}");
    assert!(job.errors.as_array().unwrap().is_empty(), "{}", job.errors);

    // The show's folder: a safe name from its title, inside the store.
    let folder = PathBuf::from(folder_of(&pool, lantern).await.expect("named"));
    assert_eq!(folder, store.join("Lantern Hour Live"));
    let eps = episodes(&pool, lantern).await;
    assert_eq!(downloaded(&eps), vec!["l3", "l4"], "newest first");
    for guid in ["l3", "l4"] {
        let e = &eps[guid];
        let path = PathBuf::from(e.file_path.as_deref().unwrap());
        assert_eq!(path.parent(), Some(folder.as_path()));
        assert!(path.is_absolute());
        assert_eq!(e.file_size_bytes, Some(tone_mp3.len() as i64));
        assert_eq!(std::fs::read(&path).unwrap(), tone_mp3);
        let ms = e.file_duration_ms.expect("probed length");
        assert!((2900..=3200).contains(&ms), "{ms}");
        assert_eq!(e.file_container.as_deref(), Some("mp3"));
        assert_eq!(e.file_codec.as_deref(), Some("mp3"));
        assert!(e.downloaded_at.is_some());
    }
    assert_eq!(
        listing(&folder),
        vec!["Four.mp3", "Three.mp3"],
        "no temp files"
    );
    for guid in ["l1", "l2"] {
        assert!(eps[guid].file_path.is_none() && eps[guid].downloaded_at.is_none());
    }

    // --------------------------------------- 2. a check with nothing new
    // "New" is after the newest download (l4), so l1/l2 stay as they are.
    let job = check(&pool, &http, lantern).await;
    let result = job.result.unwrap();
    assert_eq!(result["downloaded"], 0, "{result}");
    assert_eq!(result["removed"], 0, "2 downloaded, keep 2: {result}");
    assert_eq!(stub.hits("l1.mp3") + stub.hits("l2"), 0);

    // ------------------------------- 3. new episodes, then retention
    // Three new ones; two share a title. Listening progress on l3.
    items.push(("l5", "Five", 5, audio("l5.mp3")));
    items.push(("l6", "Same Title", 6, audio("l6.mp3")));
    items.push(("l7", "Same Title", 7, audio("l7.mp3")));
    stub.feed("lantern", feed("Lantern / Hour: Live", &items));
    let (profile,): (Uuid,) = sqlx::query_as("SELECT id FROM profiles LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let l3 = eps["l3"].id;
    let l3_path = PathBuf::from(eps["l3"].file_path.clone().unwrap());
    sqlx::query(
        "INSERT INTO book_progress (profile_id, episode_id, position_ms, duration_ms)
         VALUES ($1, $2, 1500, 3000)",
    )
    .bind(profile)
    .bind(l3)
    .execute(&pool)
    .await
    .unwrap();

    let job = check(&pool, &http, lantern).await;
    let result = job.result.unwrap();
    assert_eq!(result["new_episodes"], 3, "{result}");
    assert_eq!(result["downloaded"], 2, "{result}");
    assert_eq!(result["removed"], 1, "at most one per check: {result}");
    let eps = episodes(&pool, lantern).await;
    assert_eq!(eps.len(), 7, "retention keeps the row");
    assert_eq!(
        downloaded(&eps),
        vec!["l4", "l6", "l7"],
        "l3, the oldest, went"
    );
    // Same title: the second gets the episode's short id.
    let l6 = PathBuf::from(eps["l6"].file_path.clone().unwrap());
    let l7 = PathBuf::from(eps["l7"].file_path.clone().unwrap());
    assert_eq!(l7.file_name().unwrap(), "Same Title.mp3", "newest first");
    let short = &eps["l6"].id.simple().to_string()[..8];
    assert_eq!(
        l6.file_name().unwrap().to_string_lossy(),
        format!("Same Title [{short}].mp3")
    );
    // The removed episode: file gone, every file column cleared, the
    // row and its progress kept.
    assert!(!l3_path.exists());
    let e = &eps["l3"];
    assert!(e.file_path.is_none() && e.downloaded_at.is_none());
    assert!(e.file_size_bytes.is_none() && e.file_duration_ms.is_none());
    assert!(e.file_container.is_none() && e.file_codec.is_none());
    let (kept,): (i64,) = sqlx::query_as(
        "SELECT position_ms FROM book_progress WHERE profile_id = $1 AND episode_id = $2",
    )
    .bind(profile)
    .bind(l3)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept, 1500, "progress stays with the episode");

    // Next check: still one over, one more goes; then none.
    let result = check(&pool, &http, lantern).await.result.unwrap();
    assert_eq!(
        (result["downloaded"].clone(), result["removed"].clone()),
        (json!(0), json!(1))
    );
    let result = check(&pool, &http, lantern).await.result.unwrap();
    assert_eq!(
        (result["downloaded"].clone(), result["removed"].clone()),
        (json!(0), json!(0))
    );
    let eps = episodes(&pool, lantern).await;
    assert_eq!(downloaded(&eps), vec!["l6", "l7"]);
    assert_eq!(
        stub.hits("l3.mp3"),
        1,
        "a removed episode isn't fetched again by a check"
    );
    assert_eq!(stub.hits("l5.mp3"), 0, "l5 wasn't among the newest two");
    let mut files = listing(&folder);
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");

    // ---------------------------------------- 4. download by hand
    // A removed episode can be downloaded again by hand, and retention
    // doesn't run on a by-hand download.
    let job = download_by_hand(&pool, &http, l3).await;
    let result = job.result.unwrap();
    assert_eq!(result["downloaded"], 1, "{result}");
    assert_eq!(result["checked"], 0, "{result}");
    assert_eq!((job.progress_done, job.progress_total), (1, 1));
    let eps = episodes(&pool, lantern).await;
    assert_eq!(downloaded(&eps), vec!["l3", "l6", "l7"]);
    assert_eq!(
        eps["l3"].file_path.as_deref(),
        Some(l3_path.to_str().unwrap())
    );
    // Already downloaded: nothing fetched.
    let result = download_by_hand(&pool, &http, l3).await.result.unwrap();
    assert_eq!(result["already_downloaded"], 1, "{result}");
    assert_eq!(stub.hits("l3.mp3"), 2);
    // An extension-less URL is named by the probe.
    download_by_hand(&pool, &http, eps["l2"].id).await;
    let eps = episodes(&pool, lantern).await;
    assert!(eps["l2"]
        .file_path
        .as_deref()
        .unwrap()
        .ends_with("/Two.mp3"));

    // Ice: auto-download off. Its check downloads nothing; a by-hand job
    // downloads whatever the show says. Its title's folder is already
    // on disk (not Parlour's), so it gets the next free name.
    std::fs::create_dir(store.join("Ice Stories")).unwrap();
    let ice = add_show(&pool, pods, &format!("{host}/feeds/ice"), "Ice Stories").await;
    stub.audio(
        "i1.m4a",
        Audio {
            status: 200,
            content_type: "audio/mp4",
            body: tone_m4a.clone(),
        },
    );
    stub.feed(
        "ice",
        feed("Ice Stories", &[("i1", "Thaw", 20, audio("i1.m4a"))]),
    );
    let result = check(&pool, &http, ice).await.result.unwrap();
    assert_eq!(
        (result["new_episodes"].clone(), result["downloaded"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(
        folder_of(&pool, ice).await,
        None,
        "nothing downloaded, no folder yet"
    );
    let i1 = episodes(&pool, ice).await["i1"].id;
    let result = download_by_hand(&pool, &http, i1).await.result.unwrap();
    assert_eq!(result["downloaded"], 1, "{result}");
    let ice_folder = PathBuf::from(folder_of(&pool, ice).await.unwrap());
    assert_eq!(ice_folder, store.join("Ice Stories (2)"));
    let e = &episodes(&pool, ice).await["i1"];
    assert_eq!(
        e.file_path.as_deref(),
        Some(ice_folder.join("Thaw.m4a").to_str().unwrap())
    );
    assert_eq!(e.file_container.as_deref(), Some("mov"));
    assert_eq!(e.file_codec.as_deref(), Some("aac"));
    assert!(
        listing(&store.join("Ice Stories")).is_empty(),
        "the other folder is untouched"
    );

    // A scan of the podcasts library (the nightly one scans every
    // library) leaves the store's files alone, even a video podcast's.
    std::fs::write(folder.join("Video.mp4"), b"not really").unwrap();
    parlour_catalog::scan::scan_library(&pool, pods, None)
        .await
        .unwrap();
    let (items,): (i64,) = sqlx::query_as("SELECT count(*) FROM media_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(items, 0, "a podcasts library is never scanned");
    std::fs::remove_file(folder.join("Video.mp4")).unwrap();

    // ------------------------------------------- 5. failed downloads
    let broken = add_show(&pool, pods, &format!("{host}/feeds/broken"), "Broken Show").await;
    sqlx::query("UPDATE podcasts SET auto_download = true WHERE id = $1")
        .bind(broken)
        .execute(&pool)
        .await
        .unwrap();
    stub.audio(
        "b1.mp3",
        Audio {
            status: 404,
            content_type: "text/plain",
            body: b"gone".to_vec(),
        },
    );
    stub.audio(
        "b2.mp3",
        Audio {
            status: 200,
            content_type: "text/html",
            body: b"<html><body>Subscribe to listen</body></html>".to_vec(),
        },
    );
    stub.feed(
        "broken",
        feed(
            "Broken Show",
            &[
                ("b1", "Missing", 21, audio("b1.mp3")),
                ("b2", "Paywalled", 22, audio("b2.mp3")),
            ],
        ),
    );
    let job = check(&pool, &http, broken).await;
    let result = job.result.unwrap();
    assert_eq!(result["downloaded"], 0, "{result}");
    assert_eq!(result["download_failed"], 2, "{result}");
    let errors = job.errors.as_array().unwrap();
    assert_eq!(errors.len(), 2, "{errors:?}");
    let by_path: HashMap<&str, &str> = errors
        .iter()
        .map(|e| (e["path"].as_str().unwrap(), e["message"].as_str().unwrap()))
        .collect();
    assert_eq!(
        by_path["Broken Show — Missing"],
        "the enclosure answered HTTP 404"
    );
    assert!(
        by_path["Broken Show — Paywalled"].starts_with("the download could not be read"),
        "{by_path:?}"
    );
    let errors_text = job.errors.to_string();
    assert!(
        !errors_text.contains(&private) && !errors_text.contains("/audio/"),
        "{errors_text}"
    );
    let eps = episodes(&pool, broken).await;
    assert!(downloaded(&eps).is_empty());
    let broken_folder = PathBuf::from(folder_of(&pool, broken).await.unwrap());
    assert!(listing(&broken_folder).is_empty(), "no file, no temp file");
    // With nothing downloaded yet, "new" is after the previous check
    // (as in Audiobookshelf), so the next check doesn't retry them; a
    // by-hand download does.
    stub.audio("b1.mp3", mp3(&tone_mp3));
    let result = check(&pool, &http, broken).await.result.unwrap();
    assert_eq!(
        (
            result["downloaded"].clone(),
            result["download_failed"].clone()
        ),
        (json!(0), json!(0))
    );
    assert_eq!(stub.hits("b1.mp3"), 1);
    let result = download_by_hand(&pool, &http, eps["b1"].id)
        .await
        .result
        .unwrap();
    assert_eq!(result["downloaded"], 1, "{result}");
    assert_eq!(listing(&broken_folder), vec!["Missing.mp3"]);

    // ------------------------------------------- 6. stores that won't do
    // A podcasts library inside the films folder.
    let inside = films.join("pods");
    std::fs::create_dir(&inside).unwrap();
    let bad = library(&pool, "Bad Podcasts", "podcasts", Some(&inside)).await;
    let show = add_show(&pool, bad, &format!("{host}/feeds/ice"), "Ice Again").await;
    check(&pool, &http, show).await;
    let ep = episodes(&pool, show).await["i1"].id;
    let job = download_by_hand(&pool, &http, ep).await;
    assert_eq!(job.result.unwrap()["download_failed"], 1);
    let msg = job.errors[0]["message"].as_str().unwrap().to_string();
    assert!(msg.contains("overlaps the Films library's folder"), "{msg}");
    assert_eq!(folder_of(&pool, show).await, None);
    assert!(listing(&inside).is_empty());

    // A podcasts library without a folder.
    let none = library(&pool, "No Folder", "podcasts", None).await;
    let show = add_show(&pool, none, &format!("{host}/feeds/ice"), "Ice Elsewhere").await;
    check(&pool, &http, show).await;
    let ep = episodes(&pool, show).await["i1"].id;
    let job = download_by_hand(&pool, &http, ep).await;
    assert_eq!(
        job.errors[0]["message"],
        "the podcasts library has no folder to download into"
    );

    // A store this process can't write (as a read-only mount would be).
    // Root ignores permissions, so this part only runs as a normal user.
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    let ro = library(&pool, "Locked Podcasts", "podcasts", Some(&locked)).await;
    let show = add_show(&pool, ro, &format!("{host}/feeds/ice"), "Ice Locked").await;
    check(&pool, &http, show).await;
    let ep = episodes(&pool, show).await["i1"].id;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe_dir = locked.join("probe");
    let as_root = std::fs::create_dir(&probe_dir).is_ok();
    if as_root {
        let _ = std::fs::remove_dir(&probe_dir);
    } else {
        let job = download_by_hand(&pool, &http, ep).await;
        let msg = job.errors[0]["message"].as_str().unwrap().to_string();
        assert!(
            msg.contains("could not make") && msg.contains("Permission denied"),
            "{msg}"
        );
        assert_eq!(folder_of(&pool, show).await, None);
        let e = &episodes(&pool, show).await["i1"];
        assert!(e.file_path.is_none() && e.downloaded_at.is_none());
    }
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    // A job naming no valid episode fails; one naming a missing episode
    // does nothing.
    let id = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::RefreshPodcasts,
        None,
        json!({ "episode_id": "nope" }),
    )
    .await
    .unwrap();
    let job = parlour_jobs::claim_next(&pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .unwrap();
    parlour_server::podcasts::run_refresh_job_claimed(&pool, &http, job).await;
    assert_eq!(
        parlour_jobs::get(&pool, id).await.unwrap().unwrap().status,
        "failed"
    );
    let result = download_by_hand(&pool, &http, Uuid::new_v4())
        .await
        .result
        .unwrap();
    assert_eq!(result["downloaded"], 0);

    std::fs::remove_dir_all(&root).unwrap();
}
