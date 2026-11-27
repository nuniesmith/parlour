//! End-to-end test of POD8 (D35: playlists, collections and bookmarks)
//! against a real Postgres and the real router:
//!
//! - **playlists** are a profile's own: create in an audiobooks or a
//!   podcasts library it can reach (not a video library, not one without
//!   a grant), rename, describe, add books or episodes of that library
//!   only (no duplicates, never the other kind), reorder with the full
//!   list (a stale list is a 409), remove, delete; no other profile — an
//!   admin's included — can see or change them; removing the last entry
//!   deletes the playlist (as Audiobookshelf);
//! - **unreachable entries**: with D41's flag off an explicit show's
//!   episodes vanish from the profile's playlist (not counted, not
//!   addable, not removable, not named in a reorder) and come back in
//!   their place when it is on again; without the grant the playlist is
//!   a 404 and comes back with it; a deleted book's entry goes with it;
//! - **collections** are a library's, seen by every profile with the
//!   grant, changed only by admins (404 to anyone else): books of that
//!   audiobooks library only, ordered, an empty one kept;
//! - **bookmarks** are per profile per book: a time (clamped onto the
//!   book) and a title, one per time (a second add answers the first,
//!   retitled when a title is sent), rename, delete; never on an episode;
//! - the private token in a feed's addresses never appears in an answer.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database).
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test playlists_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// The private token every feed and enclosure address carries.
const TOKEN: &str = "fake-feed-token";

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

#[derive(Clone)]
struct Client {
    http: reqwest::Client,
    base: String,
    cookie: Option<String>,
}

impl Client {
    /// (status, JSON); every answer is checked for the token.
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = &self.cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        assert!(!text.contains(TOKEN), "{path} shows the token: {text}");
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.req(Method::GET, path, None).await;
        assert_eq!(s, 200, "{path}: {v}");
        v
    }

    async fn status(&self, method: Method, path: &str, body: Option<Value>) -> u16 {
        self.req(method, path, body).await.0
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body)).await
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::PUT, path, Some(body)).await
    }

    async fn patch(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::PATCH, path, Some(body)).await
    }

    async fn delete(&self, path: &str) -> (u16, Value) {
        self.req(Method::DELETE, path, None).await
    }
}

async fn login(http: &reqwest::Client, base: &str, user: &str) -> (String, Value) {
    let res = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": user, "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(res.status(), 200);
    let cookie = res
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .next()
                .and_then(|kv| kv.split_once('='))
                .map(|(_, v)| v.to_string())
        })
        .expect("session cookie");
    (cookie, res.json().await.unwrap())
}

async fn select(c: &Client, profile: &str) {
    let (s, v) = c
        .post(&format!("/api/v1/profiles/{profile}/select"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
}

async fn book(pool: &PgPool, library: &str, title: &str, length_ms: i64) -> String {
    let library: Uuid = library.parse().unwrap();
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO media_items (library_id, kind, stable_key, title, sort_title)
         VALUES ($1, 'audiobook', $2, $3, $3) RETURNING id",
    )
    .bind(library)
    .bind(format!("audiobook:Test/{title}"))
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_files (item_id, library_id, path, relative_path, size_bytes, mtime,
                                  container, duration_ms)
         VALUES ($1, $2, $3, $4, 1000, now(), 'ogg', $5)",
    )
    .bind(id)
    .bind(library)
    .bind(format!("/books/{title}/{title}.ogg"))
    .bind(format!("{title}/{title}.ogg"))
    .bind(length_ms)
    .execute(pool)
    .await
    .unwrap();
    id.to_string()
}

async fn show(pool: &PgPool, library: &str, title: &str, explicit: bool) -> Uuid {
    let library: Uuid = library.parse().unwrap();
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO podcasts (library_id, feed_url, title, sort_title, explicit)
         VALUES ($1, $2, $3, $3, $4) RETURNING id",
    )
    .bind(library)
    .bind(format!(
        "https://feeds.example.test/{}.xml?token={TOKEN}",
        title.replace(' ', "-")
    ))
    .bind(title)
    .bind(explicit)
    .fetch_one(pool)
    .await
    .unwrap();
    id
}

async fn episode(pool: &PgPool, show: Uuid, title: &str, day: i32) -> String {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO podcast_episodes (podcast_id, guid, title, published_at, duration_ms,
                                       enclosure_url, enclosure_type)
         VALUES ($1, $2, $2, make_timestamptz(2026, 11, $3, 7, 0, 0), 60000, $4, 'audio/mpeg')
         RETURNING id",
    )
    .bind(show)
    .bind(title)
    .bind(day)
    .bind(format!(
        "https://media.example.test/{}.mp3?token={TOKEN}",
        title.replace(' ', "-")
    ))
    .fetch_one(pool)
    .await
    .unwrap();
    id.to_string()
}

/// The entries' book or episode titles, in order.
fn entry_titles(playlist: &Value) -> Vec<String> {
    playlist["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["book"]["title"]
                .as_str()
                .or(e["episode"]["title"].as_str())
                .unwrap()
                .to_string()
        })
        .collect()
}

/// The entry id of the entry holding `title`.
fn entry_id(playlist: &Value, title: &str) -> String {
    playlist["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["book"]["title"] == title || e["episode"]["title"] == title)
        .unwrap_or_else(|| panic!("no entry {title} in {playlist}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn playlists_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-pod8-flow-{}", Uuid::new_v4().simple()));

    let app = parlour_server::http::router(AppState::new(pool.clone(), None, false));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let client = |cookie: String| Client {
        http: http.clone(),
        base: base.clone(),
        cookie: Some(cookie),
    };

    // ------------------------------------------------ people and libraries
    let (cookie, me) = login(&http, &base, "jordan").await;
    let admin = client(cookie);
    let main_profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    select(&admin, &main_profile).await;
    let (s, kid) = admin.post("/api/v1/profiles", json!({"name": "Kid"})).await;
    assert_eq!(s, 201, "{kid}");
    let kid_profile = kid["id"].as_str().unwrap().to_string();
    let kid = client(login(&http, &base, "jordan").await.0);
    select(&kid, &kid_profile).await;
    let invite = {
        let (s, inv) = admin.post("/api/v1/admin/invites", json!({})).await;
        assert_eq!(s, 201, "{inv}");
        inv["token"].as_str().unwrap().to_string()
    };
    let anon = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: None,
    };
    let (s, v) = anon
        .post(
            "/api/v1/invites/accept",
            json!({"token": invite, "username": "sam", "password": "admin-pass-123"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (sam_cookie, sam_me) = login(&http, &base, "sam").await;
    let sam = client(sam_cookie);
    let sam_profile = sam_me["profiles"][0]["id"].as_str().unwrap().to_string();
    select(&sam, &sam_profile).await;

    let mut libs = HashMap::new();
    for (name, kind) in [
        ("Books", "audiobooks"),
        ("More books", "audiobooks"),
        ("Podcasts", "podcasts"),
        ("Movies", "movies"),
    ] {
        let dir = root.join(name.replace(' ', "-"));
        std::fs::create_dir_all(&dir).unwrap();
        let (s, lib) = admin
            .post(
                "/api/v1/admin/libraries",
                json!({"name": name, "kind": kind, "paths": [dir.to_string_lossy()]}),
            )
            .await;
        assert_eq!(s, 201, "{lib}");
        libs.insert(name, lib["id"].as_str().unwrap().to_string());
    }
    let grant = |lib: &str, profile: &str| {
        let admin = admin.clone();
        let (lib, profile) = (lib.to_string(), profile.to_string());
        async move {
            let (s, v) = admin
                .post(
                    &format!("/api/v1/admin/libraries/{lib}/access"),
                    json!({"profile_id": profile}),
                )
                .await;
            assert_eq!(s, 204, "{v}");
        }
    };
    let revoke = |lib: &str, profile: &str| {
        let pool = pool.clone();
        let (lib, profile): (Uuid, Uuid) = (lib.parse().unwrap(), profile.parse().unwrap());
        async move {
            sqlx::query("DELETE FROM library_access WHERE library_id = $1 AND profile_id = $2")
                .bind(lib)
                .bind(profile)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    for p in [&main_profile, &kid_profile, &sam_profile] {
        grant(&libs["Books"], p).await;
        grant(&libs["Podcasts"], p).await;
    }
    grant(&libs["More books"], &main_profile).await;
    grant(&libs["Movies"], &main_profile).await;

    let alpha = book(&pool, &libs["Books"], "Alpha", 100_000).await;
    let bravo = book(&pool, &libs["Books"], "Bravo", 200_000).await;
    let charlie = book(&pool, &libs["Books"], "Charlie", 300_000).await;
    let elsewhere = book(&pool, &libs["More books"], "Elsewhere", 50_000).await;
    let clean = show(&pool, &libs["Podcasts"], "Clean Show", false).await;
    let spicy = show(&pool, &libs["Podcasts"], "Spicy Show", true).await;
    let e1 = episode(&pool, clean, "Clean one", 10).await;
    let e2 = episode(&pool, clean, "Clean two", 11).await;
    let e3 = episode(&pool, spicy, "Spicy one", 12).await;

    // ------------------------------------------------ 1. a book playlist
    let books_lib = libs["Books"].clone();
    let (s, v) = admin
        .post(
            "/api/v1/playlists",
            json!({"library_id": books_lib, "name": "   "}),
        )
        .await;
    assert_eq!(s, 400, "a blank name: {v}");
    let (s, _) = admin
        .post(
            "/api/v1/playlists",
            json!({"library_id": libs["Movies"], "name": "Films"}),
        )
        .await;
    assert_eq!(s, 400, "not a video library");
    let (s, _) = kid
        .post(
            "/api/v1/playlists",
            json!({"library_id": libs["More books"], "name": "Nope"}),
        )
        .await;
    assert_eq!(s, 404, "no grant, no library");
    let (s, _) = admin
        .post(
            "/api/v1/playlists",
            json!({"library_id": books_lib, "name": "X", "colour": "red"}),
        )
        .await;
    assert_eq!(s, 422, "an unknown field");
    let (s, trip) = admin
        .post(
            "/api/v1/playlists",
            json!({"library_id": books_lib, "name": " Road trip ", "description": "For the car"}),
        )
        .await;
    assert_eq!(s, 201, "{trip}");
    assert_eq!(trip["name"], "Road trip");
    assert_eq!(trip["library_kind"], "audiobooks");
    assert_eq!(trip["item_count"], 0);
    assert_eq!(trip["items"], json!([]));
    let trip_id = trip["id"].as_str().unwrap().to_string();
    let items = format!("/api/v1/playlists/{trip_id}/items");

    for b in [&alpha, &bravo, &charlie] {
        let (s, v) = admin.post(&items, json!({"item_id": b})).await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, _) = admin.post(&items, json!({"item_id": alpha})).await;
    assert_eq!(s, 409, "already in it");
    let (s, v) = admin.post(&items, json!({"item_id": elsewhere})).await;
    assert_eq!(s, 400, "another library's book: {v}");
    let (s, _) = admin.post(&items, json!({"episode_id": e1})).await;
    assert_eq!(s, 400, "an episode in a book playlist");
    let (s, _) = admin.post(&items, json!({})).await;
    assert_eq!(s, 400, "neither");
    let (s, _) = admin
        .post(&items, json!({"item_id": alpha, "episode_id": e1}))
        .await;
    assert_eq!(s, 400, "both");
    let (s, _) = admin
        .post(&items, json!({"item_id": Uuid::new_v4().to_string()}))
        .await;
    assert_eq!(s, 404, "no such book");
    let trip = admin.get(&format!("/api/v1/playlists/{trip_id}")).await;
    assert_eq!(entry_titles(&trip), ["Alpha", "Bravo", "Charlie"]);
    assert_eq!(trip["item_count"], 3);
    assert_eq!(trip["duration_ms"], 600_000);
    assert_eq!(trip["items"][0]["kind"], "book");
    assert_eq!(trip["items"][0]["book"]["progress"]["position_ms"], 0);

    let list = admin
        .get(&format!(
            "/api/v1/playlists?library_id={books_lib}&item_id={alpha}"
        ))
        .await;
    assert_eq!(names(&list), ["Road trip"]);
    assert_eq!(list[0]["contains"], true);
    let list = admin
        .get(&format!("/api/v1/playlists?item_id={elsewhere}"))
        .await;
    assert_eq!(list[0]["contains"], false);

    // Reorder: every entry once, else 409.
    let order = |p: &Value, titles: &[&str]| -> Vec<String> {
        titles.iter().map(|t| entry_id(p, t)).collect()
    };
    let (s, v) = admin
        .put(
            &items,
            json!({"items": order(&trip, &["Charlie", "Alpha", "Bravo"])}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(entry_titles(&v), ["Charlie", "Alpha", "Bravo"]);
    let (s, _) = admin
        .put(
            &items,
            json!({"items": order(&trip, &["Charlie", "Alpha"])}),
        )
        .await;
    assert_eq!(s, 409, "a short list");
    let (s, _) = admin
        .put(
            &items,
            json!({"items": order(&trip, &["Charlie", "Alpha", "Alpha"])}),
        )
        .await;
    assert_eq!(s, 409, "a repeat");

    // Rename and describe.
    let (s, v) = admin
        .patch(
            &format!("/api/v1/playlists/{trip_id}"),
            json!({"name": "Long drive"}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["name"], "Long drive");
    assert_eq!(v["description"], "For the car", "kept when not sent");
    let (s, v) = admin
        .patch(
            &format!("/api/v1/playlists/{trip_id}"),
            json!({"description": ""}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["description"], Value::Null, "cleared");
    assert_eq!(v["name"], "Long drive");

    // Nobody else's: another profile of the same admin, and another user.
    for other in [&kid, &sam] {
        assert_eq!(
            other
                .status(Method::GET, &format!("/api/v1/playlists/{trip_id}"), None)
                .await,
            404
        );
        assert_eq!(other.get("/api/v1/playlists").await, json!([]));
        assert_eq!(
            other
                .status(
                    Method::PATCH,
                    &format!("/api/v1/playlists/{trip_id}"),
                    Some(json!({"name": "Mine"}))
                )
                .await,
            404
        );
        assert_eq!(
            other
                .status(Method::POST, &items, Some(json!({"item_id": alpha})))
                .await,
            404
        );
        assert_eq!(
            other
                .status(
                    Method::DELETE,
                    &format!("/api/v1/playlists/{trip_id}"),
                    None
                )
                .await,
            404
        );
    }

    // A deleted book's entry goes with it.
    sqlx::query("DELETE FROM media_items WHERE id = $1")
        .bind(bravo.parse::<Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let trip = admin.get(&format!("/api/v1/playlists/{trip_id}")).await;
    assert_eq!(entry_titles(&trip), ["Charlie", "Alpha"]);

    // ------------------------------- 2. an episode playlist, D41, grants
    let pods = libs["Podcasts"].clone();
    let (s, commute) = kid
        .post(
            "/api/v1/playlists",
            json!({"library_id": pods, "name": "Commute"}),
        )
        .await;
    assert_eq!(s, 201, "{commute}");
    assert_eq!(commute["library_kind"], "podcasts");
    let commute_id = commute["id"].as_str().unwrap().to_string();
    let citems = format!("/api/v1/playlists/{commute_id}/items");
    for e in [&e3, &e1, &e2] {
        let (s, v) = kid.post(&citems, json!({"episode_id": e})).await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, _) = kid.post(&citems, json!({"item_id": alpha})).await;
    assert_eq!(s, 400, "a book in an episode playlist");
    let c = kid
        .get(&format!("/api/v1/playlists/{commute_id}?client=web"))
        .await;
    assert_eq!(entry_titles(&c), ["Spicy one", "Clean one", "Clean two"]);
    assert_eq!(c["items"][0]["kind"], "episode");
    assert_eq!(c["items"][0]["episode"]["delivery"], "enclosure");
    assert!(c["items"][0]["episode"]["stream_url"]
        .as_str()
        .unwrap()
        .starts_with(&format!("/api/v1/podcast-episodes/{e3}/stream?token=")));
    assert_eq!(c["duration_ms"], 180_000);
    let spicy_entry = entry_id(&c, "Spicy one");

    // D41 off: the explicit show's episode is out of sight, kept.
    let (s, v) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let c = kid.get(&format!("/api/v1/playlists/{commute_id}")).await;
    assert_eq!(entry_titles(&c), ["Clean one", "Clean two"]);
    assert_eq!(c["item_count"], 2);
    assert_eq!(c["duration_ms"], 120_000);
    let listed = kid
        .get(&format!(
            "/api/v1/playlists?library_id={pods}&episode_id={e3}"
        ))
        .await;
    assert_eq!(listed[0]["item_count"], 2);
    assert_eq!(listed[0]["contains"], false, "nothing said of it");
    let (s, _) = kid.post(&citems, json!({"episode_id": e3})).await;
    assert_eq!(s, 404, "an explicit episode can't be added");
    let (s, _) = kid.delete(&format!("{citems}/{spicy_entry}")).await;
    assert_eq!(s, 404, "nor removed");
    let mut with_hidden = order(&c, &["Clean two", "Clean one"]);
    with_hidden.push(spicy_entry.clone());
    let (s, _) = kid.put(&citems, json!({"items": with_hidden})).await;
    assert_eq!(s, 409, "nor named in a reorder");
    let (s, v) = kid
        .put(
            &citems,
            json!({"items": order(&c, &["Clean two", "Clean one"])}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(entry_titles(&v), ["Clean two", "Clean one"]);
    // Back on: it returns in its own place.
    let (s, _) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": true}),
        )
        .await;
    assert_eq!(s, 200);
    let c = kid.get(&format!("/api/v1/playlists/{commute_id}")).await;
    assert_eq!(entry_titles(&c), ["Spicy one", "Clean two", "Clean one"]);

    // Without the grant the playlist is out of reach; it comes back.
    revoke(&pods, &kid_profile).await;
    assert_eq!(
        kid.status(
            Method::GET,
            &format!("/api/v1/playlists/{commute_id}"),
            None
        )
        .await,
        404
    );
    assert_eq!(kid.get("/api/v1/playlists").await, json!([]));
    grant(&pods, &kid_profile).await;
    let c = kid.get(&format!("/api/v1/playlists/{commute_id}")).await;
    assert_eq!(c["item_count"], 3);

    // Removing entries; the last one takes the playlist with it (ABS).
    for (title, last) in [
        ("Clean one", false),
        ("Spicy one", false),
        ("Clean two", true),
    ] {
        let c = kid.get(&format!("/api/v1/playlists/{commute_id}")).await;
        let (s, v) = kid
            .delete(&format!("{citems}/{}", entry_id(&c, title)))
            .await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["playlist_deleted"], last, "{title}");
    }
    assert_eq!(
        kid.status(
            Method::GET,
            &format!("/api/v1/playlists/{commute_id}"),
            None
        )
        .await,
        404
    );

    // Deleting a playlist.
    let (s, _) = admin.delete(&format!("/api/v1/playlists/{trip_id}")).await;
    assert_eq!(s, 204);
    assert_eq!(
        admin
            .status(Method::GET, &format!("/api/v1/playlists/{trip_id}"), None)
            .await,
        404
    );

    // ------------------------------------------------ 3. collections
    let (s, _) = sam
        .post(
            "/api/v1/admin/collections",
            json!({"library_id": books_lib, "name": "Mine"}),
        )
        .await;
    assert_eq!(s, 404, "admins only");
    let (s, _) = admin
        .post(
            "/api/v1/admin/collections",
            json!({"library_id": pods, "name": "Shows"}),
        )
        .await;
    assert_eq!(s, 400, "books only");
    let (s, _) = admin
        .post(
            "/api/v1/admin/collections",
            json!({"library_id": Uuid::new_v4().to_string(), "name": "Shows"}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, fav) = admin
        .post(
            "/api/v1/admin/collections",
            json!({"library_id": books_lib, "name": "Favourites"}),
        )
        .await;
    assert_eq!(s, 201, "{fav}");
    let fav_id = fav["id"].as_str().unwrap().to_string();
    let fitems = format!("/api/v1/admin/collections/{fav_id}/items");
    for b in [&alpha, &charlie] {
        let (s, v) = admin.post(&fitems, json!({"item_id": b})).await;
        assert_eq!(s, 201, "{v}");
    }
    let (s, _) = admin.post(&fitems, json!({"item_id": alpha})).await;
    assert_eq!(s, 409);
    let (s, _) = admin.post(&fitems, json!({"item_id": elsewhere})).await;
    assert_eq!(s, 400, "another library's book");
    let (s, _) = admin.post(&fitems, json!({"item_id": e1})).await;
    assert_eq!(s, 404, "an episode is no book");
    for (method, path, body) in [
        (
            Method::PATCH,
            format!("/api/v1/admin/collections/{fav_id}"),
            Some(json!({"name": "Ours"})),
        ),
        (
            Method::DELETE,
            format!("/api/v1/admin/collections/{fav_id}"),
            None,
        ),
        (
            Method::POST,
            fitems.clone(),
            Some(json!({"item_id": alpha})),
        ),
        (Method::PUT, fitems.clone(), Some(json!({"items": []}))),
        (Method::DELETE, format!("{fitems}/{alpha}"), None),
    ] {
        assert_eq!(sam.status(method, &path, body).await, 404, "{path}");
    }

    // Everyone with the grant sees it, with their own progress.
    let (s, _) = sam
        .put(
            &format!("/api/v1/audiobooks/{charlie}/progress"),
            json!({"position_ms": 60_000}),
        )
        .await;
    assert_eq!(s, 200);
    let seen = sam
        .get(&format!(
            "/api/v1/collections?library_id={books_lib}&item_id={charlie}"
        ))
        .await;
    assert_eq!(names(&seen), ["Favourites"]);
    assert_eq!(seen[0]["book_count"], 2);
    assert_eq!(seen[0]["contains"], true);
    let f = sam.get(&format!("/api/v1/collections/{fav_id}")).await;
    let titles: Vec<&str> = f["books"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Alpha", "Charlie"]);
    assert_eq!(f["books"][1]["progress"]["position_ms"], 60_000);
    assert_eq!(
        kid.get(&format!("/api/v1/collections/{fav_id}")).await["books"][1]["progress"]
            ["position_ms"],
        0,
        "per profile"
    );
    // Reorder, rename: everyone sees it.
    let (s, v) = admin.put(&fitems, json!({"items": [charlie, alpha]})).await;
    assert_eq!(s, 200, "{v}");
    let (s, _) = admin.put(&fitems, json!({"items": [charlie]})).await;
    assert_eq!(s, 409);
    let (s, _) = admin
        .patch(
            &format!("/api/v1/admin/collections/{fav_id}"),
            json!({"name": "Staff picks", "description": "Read these"}),
        )
        .await;
    assert_eq!(s, 200);
    let f = sam.get(&format!("/api/v1/collections/{fav_id}")).await;
    assert_eq!(f["name"], "Staff picks");
    assert_eq!(f["description"], "Read these");
    assert_eq!(f["books"][0]["title"], "Charlie");
    // No grant, no collection.
    revoke(&books_lib, &sam_profile).await;
    assert_eq!(
        sam.status(Method::GET, &format!("/api/v1/collections/{fav_id}"), None)
            .await,
        404
    );
    assert_eq!(sam.get("/api/v1/collections").await, json!([]));
    grant(&books_lib, &sam_profile).await;
    // Remove books; an empty collection stays.
    for b in [&alpha, &charlie] {
        let (s, _) = admin.delete(&format!("{fitems}/{b}")).await;
        assert_eq!(s, 204);
    }
    let (s, _) = admin.delete(&format!("{fitems}/{alpha}")).await;
    assert_eq!(s, 404);
    let f = sam.get(&format!("/api/v1/collections/{fav_id}")).await;
    assert_eq!(f["book_count"], 0);
    let (s, _) = admin
        .delete(&format!("/api/v1/admin/collections/{fav_id}"))
        .await;
    assert_eq!(s, 204);
    assert_eq!(
        sam.status(Method::GET, &format!("/api/v1/collections/{fav_id}"), None)
            .await,
        404
    );

    // ------------------------------------------------ 4. bookmarks
    let marks = format!("/api/v1/audiobooks/{alpha}/bookmarks");
    let (s, m) = admin
        .post(&marks, json!({"position_ms": 30_000, "title": " Twist "}))
        .await;
    assert_eq!(s, 201, "{m}");
    assert_eq!(m["title"], "Twist");
    let twist = m["id"].as_str().unwrap().to_string();
    let (s, m) = admin.post(&marks, json!({"position_ms": 30_000})).await;
    assert_eq!(s, 200, "the same time answers the first: {m}");
    assert_eq!(
        (m["id"].as_str().unwrap(), &m["title"]),
        (twist.as_str(), &json!("Twist"))
    );
    let (s, m) = admin
        .post(&marks, json!({"position_ms": 30_000, "title": "Big twist"}))
        .await;
    assert_eq!(s, 200);
    assert_eq!(m["title"], "Big twist");
    let (s, m) = admin.post(&marks, json!({"position_ms": 999_999})).await;
    assert_eq!(s, 201);
    assert_eq!(m["position_ms"], 100_000, "clamped onto the book");
    assert_eq!(m["title"], "");
    let end = m["id"].as_str().unwrap().to_string();
    let (s, _) = admin.post(&marks, json!({"position_ms": 5_000})).await;
    assert_eq!(s, 201);
    let (s, _) = admin.post(&marks, json!({"position_ms": -1})).await;
    assert_eq!(s, 400);
    let (s, _) = admin
        .post(&marks, json!({"position_ms": 1, "title": "x".repeat(201)}))
        .await;
    assert_eq!(s, 400);
    let all = admin.get(&marks).await;
    let times: Vec<i64> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["position_ms"].as_i64().unwrap())
        .collect();
    assert_eq!(times, [5_000, 30_000, 100_000], "by time");
    let (s, m) = admin
        .patch(&format!("{marks}/{end}"), json!({"title": "The end"}))
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        (&m["title"], &m["position_ms"]),
        (&json!("The end"), &json!(100_000))
    );
    // Per profile, and only through the grant.
    assert_eq!(kid.get(&marks).await, json!([]));
    assert_eq!(
        kid.status(
            Method::PATCH,
            &format!("{marks}/{end}"),
            Some(json!({"title": "Mine"}))
        )
        .await,
        404
    );
    assert_eq!(
        kid.status(Method::DELETE, &format!("{marks}/{end}"), None)
            .await,
        404
    );
    revoke(&books_lib, &main_profile).await;
    assert_eq!(admin.status(Method::GET, &marks, None).await, 404);
    grant(&books_lib, &main_profile).await;
    assert_eq!(admin.get(&marks).await.as_array().unwrap().len(), 3, "kept");
    // Books only.
    assert_eq!(
        admin
            .status(
                Method::POST,
                &format!("/api/v1/audiobooks/{e1}/bookmarks"),
                Some(json!({"position_ms": 1}))
            )
            .await,
        404
    );
    let (s, _) = admin.delete(&format!("{marks}/{twist}")).await;
    assert_eq!(s, 204);
    let (s, _) = admin.delete(&format!("{marks}/{twist}")).await;
    assert_eq!(s, 404);
    assert_eq!(admin.get(&marks).await.as_array().unwrap().len(), 2);

    // Nothing anywhere carries the token: the tables POD8 added included.
    let (n,): (i64,) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM playlists t WHERE t::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM playlist_items t WHERE t::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM collections t WHERE t::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM collection_items t WHERE t::text LIKE '%' || $1 || '%')
              + (SELECT count(*) FROM bookmarks t WHERE t::text LIKE '%' || $1 || '%')",
    )
    .bind(TOKEN)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "the token in a POD8 table");
    let _ = std::fs::remove_dir_all(&root);
}
