//! XFER2: turning database rows into a bundle — names for ids, the
//! spec's fixed order, the hash confirm and the artwork opt-in — and
//! the zip on disk, read back through `BundleReader`.
//!
//! Everything here is fictional: the hashes are syntactically valid
//! Argon2id strings whose salt and "hash" decode to `fixture-salt-…`
//! / `fixture-only-…` text, which no password produces.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::*;
use crate::bundle::{BundleReader, BundleWriter, ItemKind, SegmentSource};

const PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$Zml4dHVyZS1zYWx0LXhmZXIy$Zml4dHVyZS1vbmx5LXhmZXIyLXBhc3N3b3JkLWhhc2g";
const PIN_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$Zml4dHVyZS1zYWx0LXhmZXIyLXBpbg$Zml4dHVyZS1vbmx5LXhmZXIyLXBpbi1oYXNoLTAwMDA";

const POSTER: &[u8] = b"fixture poster bytes, not really a jpeg";
const STILL: &[u8] = b"fixture still bytes, not really a png";

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn at(day: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, 12, 0, second).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Ids: users 1x, profiles 2x, libraries 3x, items 4x.
const ADA: u128 = 11;
const BEN: u128 = 12;
const ADA_P: u128 = 21;
const KIDS_P: u128 = 22;
const BEN_P: u128 = 23;
const FILMS: u128 = 31;
const SERIES: u128 = 32;
const MOVIE: u128 = 41;
const SHOW: u128 = 42;
const SEASON: u128 = 43;
const EPISODE: u128 = 44;

fn user(uid: u128, name: &str, admin: bool) -> UserRow {
    UserRow {
        id: id(uid),
        username: name.into(),
        password_hash: PASSWORD_HASH.into(),
        is_admin: admin,
        created_at: at(1, uid as u32),
        disabled_at: None,
    }
}

fn profile(pid: u128, uid: u128, name: &str, created: DateTime<Utc>, pin: bool) -> ProfileRow {
    ProfileRow {
        id: id(pid),
        user_id: id(uid),
        name: name.into(),
        pin_hash: pin.then(|| PIN_HASH.to_string()),
        avatar_color: Some("#4a7".into()),
        audio_lang: Some("eng".into()),
        subtitle_lang: None,
        subtitle_mode: "auto".into(),
        remote_bitrate_cap_kbps: None,
        created_at: created,
        home_rail_order: vec!["continue".into()],
        home_rails_hidden: vec![],
        segment_skip: "button".into(),
        autoplay_next: true,
    }
}

fn item(iid: u128, lib: u128, parent: Option<u128>, kind: &str, key: &str) -> ItemRow {
    ItemRow {
        id: id(iid),
        library_id: id(lib),
        parent_id: parent.map(id),
        kind: kind.into(),
        stable_key: key.into(),
        title: key.into(),
        sort_title: key.into(),
        year: Some(2021),
        season_number: None,
        episode_number: None,
        overview: None,
        content_rating: Some("PG".into()),
        tmdb_id: Some(iid as i64),
        tvdb_id: None,
        imdb_id: None,
        created_at: at(2, iid as u32),
        updated_at: at(3, iid as u32),
        genres: vec!["Drama".into()],
        vote_average: Some(7.5),
        runtime_minutes: Some(90),
        air_date: None,
        metadata_status: "matched".into(),
        metadata_provider: Some("tmdb".into()),
        last_metadata_at: Some(at(3, 0)),
    }
}

fn image(iid: u128, kind: &str, variant: &str, path: &str, hash: Option<&str>) -> ImageRow {
    ImageRow {
        item_id: id(iid),
        kind: kind.into(),
        variant: variant.into(),
        source_url: format!("https://images.example.org/{iid}/{kind}-{variant}.jpg"),
        path: path.into(),
        hash: hash.map(str::to_string),
    }
}

fn segment(iid: u128, kind: &str, source: &str, times: Option<(i64, i64)>) -> SegmentRow {
    SegmentRow {
        item_id: id(iid),
        kind: kind.into(),
        source: source.into(),
        start_ms: times.map(|t| t.0),
        end_ms: times.map(|t| t.1),
        confidence: 0.9,
        created_at: at(4, 0),
        updated_at: at(4, 1),
    }
}

fn watched(pid: u128, iid: u128) -> UserDataRow {
    UserDataRow {
        profile_id: id(pid),
        item_id: id(iid),
        position_ms: 1000,
        duration_ms: Some(5000),
        played: false,
        is_favorite: true,
        play_count: 1,
        last_played_at: Some(at(5, iid as u32)),
        created_at: at(5, 0),
        updated_at: at(5, 1),
    }
}

/// A small server with something in every carried table, the rows in
/// no particular order.
fn snapshot() -> Snapshot {
    Snapshot {
        taken_at: Utc.with_ymd_and_hms(2026, 11, 24, 20, 0, 0).unwrap(),
        users: vec![user(BEN, "ben", false), user(ADA, "ada", true)],
        profiles: vec![
            profile(KIDS_P, ADA, "Kids", at(1, 30), true),
            profile(BEN_P, BEN, "Ben", at(1, 20), false),
            profile(ADA_P, ADA, "Ada", at(1, 10), false),
        ],
        recent_searches: vec![
            RecentSearchRow {
                profile_id: id(ADA_P),
                query: "older".into(),
                searched_at: at(6, 1),
            },
            RecentSearchRow {
                profile_id: id(ADA_P),
                query: "newer".into(),
                searched_at: at(6, 2),
            },
        ],
        libraries: vec![
            LibraryRow {
                id: id(SERIES),
                name: "Series".into(),
                kind: "shows".into(),
                created_at: at(1, 0),
            },
            LibraryRow {
                id: id(FILMS),
                name: "Films".into(),
                kind: "movies".into(),
                created_at: at(1, 0),
            },
        ],
        paths: vec![
            LibraryPathRow {
                library_id: id(SERIES),
                path: "/media/series-b".into(),
            },
            LibraryPathRow {
                library_id: id(SERIES),
                path: "/media/series-a".into(),
            },
            LibraryPathRow {
                library_id: id(FILMS),
                path: "/media/films".into(),
            },
        ],
        access: vec![
            AccessRow {
                library_id: id(FILMS),
                profile_id: id(KIDS_P),
                max_rating: Some("PG".into()),
            },
            AccessRow {
                library_id: id(FILMS),
                profile_id: id(ADA_P),
                max_rating: None,
            },
        ],
        items: vec![
            item(
                EPISODE,
                SERIES,
                Some(SEASON),
                "episode",
                "series:harbor:2019:s01:e001",
            ),
            item(MOVIE, FILMS, None, "movie", "movie:lanterns:lanterns:2021"),
            item(
                SEASON,
                SERIES,
                Some(SHOW),
                "season",
                "series:harbor:2019:s01",
            ),
            item(SHOW, SERIES, None, "series", "series:harbor:2019"),
        ],
        credits: vec![
            CreditRow {
                item_id: id(MOVIE),
                name: "Tomas Reyes".into(),
                tmdb_id: Some(2),
                role: "actor".into(),
                character_name: None,
                sort_order: 1,
            },
            CreditRow {
                item_id: id(MOVIE),
                name: "Juniper Hale".into(),
                tmdb_id: Some(1),
                role: "actor".into(),
                character_name: Some("Mara".into()),
                sort_order: 0,
            },
        ],
        segments: vec![
            segment(EPISODE, "credits", "chapters", Some((80_000, 90_000))),
            segment(EPISODE, "intro", "manual", None),
            segment(EPISODE, "intro", "fingerprint", Some((1_000, 31_000))),
        ],
        images: vec![
            image(
                MOVIE,
                "backdrop",
                "original",
                "pending:backdrop:original",
                None,
            ),
            image(
                MOVIE,
                "poster",
                "w300",
                "/artwork/m/poster-w300.JPG",
                Some(&sha(POSTER)),
            ),
            image(
                MOVIE,
                "poster",
                "original",
                "/artwork/m/poster-original.jpg",
                Some(&sha(POSTER)),
            ),
            image(
                EPISODE,
                "still",
                "original",
                "/artwork/e/still-original.png",
                Some(&sha(STILL)),
            ),
        ],
        user_data: vec![
            watched(BEN_P, EPISODE),
            watched(ADA_P, EPISODE),
            watched(ADA_P, MOVIE),
        ],
        activity: vec![
            ActivityRow {
                id: id(902),
                created_at: at(7, 5),
                kind: "playback.start".into(),
                user_id: Some(id(BEN)),
                profile_id: Some(id(BEN_P)),
                item_id: Some(id(EPISODE)),
                actor: Some("ben".into()),
                summary: "ben started an episode".into(),
                detail: serde_json::json!({"client": "web"}),
            },
            ActivityRow {
                id: id(901),
                created_at: at(7, 1),
                kind: "scan.completed".into(),
                user_id: None,
                profile_id: None,
                item_id: None,
                actor: None,
                summary: "Scan of Films finished".into(),
                detail: serde_json::json!({"library_id": id(FILMS)}),
            },
        ],
    }
}

fn off() -> ExportOptions {
    ExportOptions::default()
}

fn with_hashes() -> ExportOptions {
    ExportOptions {
        include_hashes: true,
        include_artwork: false,
    }
}

fn with_artwork() -> ExportOptions {
    ExportOptions {
        include_hashes: false,
        include_artwork: true,
    }
}

/// The artwork map `export` builds once the cached files check out:
/// every cached row of the snapshot, named.
fn named_artwork(s: &Snapshot) -> HashMap<ArtworkKey, String> {
    s.images
        .iter()
        .filter_map(cached_key)
        .filter_map(|(path, hash)| {
            let name = artwork_name(&path, &hash)?;
            Some(((path, hash), name))
        })
        .collect()
}

fn zip_of(bundle: &Bundle, artwork: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = BundleWriter::new(Cursor::new(Vec::new()), bundle).expect("writer");
    for (name, bytes) in artwork {
        writer
            .add_artwork(name, &mut Cursor::new(*bytes))
            .expect("artwork");
    }
    writer.finish().expect("finish").into_inner()
}

// ------------------------------------------------------------ the documents

#[test]
fn a_snapshot_becomes_a_consistent_bundle_that_reads_back_the_same() {
    let bundle = build(&snapshot(), off(), &HashMap::new()).expect("build");
    assert!(bundle.problems().is_empty(), "{:?}", bundle.problems());
    let (reader, read) = BundleReader::open(Cursor::new(zip_of(&bundle, &[]))).expect("open");
    assert_eq!(read, bundle);
    assert_eq!(reader.artwork_names().count(), 0);
    assert_eq!(bundle.manifest.exported_at, snapshot().taken_at);
    assert_eq!(bundle.catalog.items.len(), 4);
    assert_eq!(bundle.user_data.activity.len(), 2);
}

#[test]
fn ids_become_names() {
    let bundle = build(&snapshot(), off(), &HashMap::new()).expect("build");
    let films = &bundle.libraries.libraries[0];
    assert_eq!(films.name, "Films");
    assert_eq!(
        films
            .access
            .iter()
            .map(|g| (
                g.username.as_str(),
                g.profile.as_str(),
                g.max_rating.as_deref()
            ))
            .collect::<Vec<_>>(),
        [("ada", "Ada", None), ("ada", "Kids", Some("PG"))]
    );
    let episode = bundle
        .catalog
        .items
        .iter()
        .find(|i| i.kind == ItemKind::Episode)
        .unwrap();
    assert_eq!(episode.library, "Series");
    assert_eq!(episode.parent.as_deref(), Some("series:harbor:2019:s01"));
    let start = &bundle.user_data.activity[1];
    assert_eq!(start.username.as_deref(), Some("ben"));
    let profile = start.profile.as_ref().unwrap();
    assert_eq!(
        (profile.username.as_str(), profile.profile.as_str()),
        ("ben", "Ben")
    );
    let item = start.item.as_ref().unwrap();
    assert_eq!(item.stable_key, "series:harbor:2019:s01:e001");
    // `detail` is carried verbatim, ids and all (D25).
    let scan = &bundle.user_data.activity[0];
    assert_eq!(scan.detail["library_id"], serde_json::json!(id(FILMS)));
    assert!(scan.username.is_none() && scan.profile.is_none() && scan.item.is_none());
}

#[test]
fn every_list_is_in_the_spec_order() {
    let bundle = build(&snapshot(), off(), &HashMap::new()).expect("build");
    let users: Vec<_> = bundle
        .users
        .users
        .iter()
        .map(|u| u.username.as_str())
        .collect();
    assert_eq!(users, ["ada", "ben"]);
    // By username, then created_at: Ada (…:10) before Kids (…:30).
    let profiles: Vec<_> = bundle
        .profiles
        .profiles
        .iter()
        .map(|p| (p.username.as_str(), p.name.as_str()))
        .collect();
    assert_eq!(profiles, [("ada", "Ada"), ("ada", "Kids"), ("ben", "Ben")]);
    let searches: Vec<_> = bundle.profiles.profiles[0]
        .recent_searches
        .iter()
        .map(|s| s.query.as_str())
        .collect();
    assert_eq!(searches, ["newer", "older"], "newest first");
    let libraries: Vec<_> = bundle
        .libraries
        .libraries
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(libraries, ["Films", "Series"]);
    assert_eq!(
        bundle.libraries.libraries[1].paths,
        ["/media/series-a", "/media/series-b"]
    );
    let items: Vec<_> = bundle
        .catalog
        .items
        .iter()
        .map(|i| (i.library.as_str(), i.stable_key.as_str()))
        .collect();
    assert_eq!(
        items,
        [
            ("Films", "movie:lanterns:lanterns:2021"),
            ("Series", "series:harbor:2019"),
            ("Series", "series:harbor:2019:s01"),
            ("Series", "series:harbor:2019:s01:e001"),
        ]
    );
    let movie = &bundle.catalog.items[0];
    let people: Vec<_> = movie.people.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(people, ["Juniper Hale", "Tomas Reyes"], "by sort_order");
    let images: Vec<_> = movie
        .images
        .iter()
        .map(|i| (i.kind.as_str(), i.variant.as_str()))
        .collect();
    assert_eq!(
        images,
        [
            ("poster", "original"),
            ("poster", "w300"),
            ("backdrop", "original")
        ],
        "kinds in the format's order, then variant"
    );
    let segments: Vec<_> = bundle.catalog.items[3]
        .segments
        .iter()
        .map(|s| (s.kind.as_str(), s.source))
        .collect();
    assert_eq!(
        segments,
        [
            ("intro", SegmentSource::Fingerprint),
            ("intro", SegmentSource::Manual),
            ("credits", SegmentSource::Chapters),
        ]
    );
    let groups: Vec<_> = bundle
        .user_data
        .profiles
        .iter()
        .map(|g| {
            (
                g.profile.as_str(),
                g.items
                    .iter()
                    .map(|r| r.item.library.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        [("Ada", vec!["Films", "Series"]), ("Ben", vec!["Series"])]
    );
    let activity: Vec<_> = bundle
        .user_data
        .activity
        .iter()
        .map(|a| a.kind.as_str())
        .collect();
    assert_eq!(
        activity,
        ["scan.completed", "playback.start"],
        "oldest first"
    );
}

#[test]
fn two_exports_of_the_same_rows_are_identical_whatever_order_postgres_returns() {
    let first = zip_of(&build(&snapshot(), off(), &HashMap::new()).unwrap(), &[]);
    let mut s = snapshot();
    s.users.reverse();
    s.profiles.reverse();
    s.recent_searches.reverse();
    s.libraries.reverse();
    s.paths.reverse();
    s.access.reverse();
    s.items.reverse();
    s.credits.reverse();
    s.segments.reverse();
    s.images.reverse();
    s.user_data.reverse();
    s.activity.reverse();
    let second = zip_of(&build(&s, off(), &HashMap::new()).unwrap(), &[]);
    assert!(first == second, "the zips differ");
}

// ------------------------------------------------------------ hashes

#[test]
fn without_the_confirm_no_hash_leaves_the_server() {
    let bundle = build(&snapshot(), off(), &HashMap::new()).expect("build");
    assert!(!bundle.manifest.hashes_included);
    for user in &bundle.users.users {
        assert!(user.password_hash.is_none() && user.pin_hashes.is_none());
    }
    // `has_pin` still says which profiles have one.
    let pins: Vec<_> = bundle
        .profiles
        .profiles
        .iter()
        .map(|p| (p.name.as_str(), p.has_pin))
        .collect();
    assert_eq!(pins, [("Ada", false), ("Kids", true), ("Ben", false)]);
    // Nowhere in the written zip, either.
    let bytes = zip_of(&bundle, &[]);
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().expect("entry name").to_string();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
        assert!(!text.contains("$argon2"), "a hash is in {name}");
        assert!(!text.contains("password_hash") && !text.contains("pin_hashes"));
    }
}

#[test]
fn with_the_confirm_every_hash_goes_in() {
    let bundle = build(&snapshot(), with_hashes(), &HashMap::new()).expect("build");
    assert!(bundle.manifest.hashes_included);
    let ada = &bundle.users.users[0];
    assert_eq!(ada.password_hash.as_deref(), Some(PASSWORD_HASH));
    assert_eq!(
        ada.pin_hashes,
        Some(BTreeMap::from([("Kids".to_string(), PIN_HASH.to_string())]))
    );
    let ben = &bundle.users.users[1];
    assert_eq!(ben.password_hash.as_deref(), Some(PASSWORD_HASH));
    assert_eq!(
        ben.pin_hashes,
        Some(BTreeMap::new()),
        "{{}} when no profile has a PIN"
    );
    let (_, read) = BundleReader::open(Cursor::new(zip_of(&bundle, &[]))).expect("open");
    assert_eq!(read, bundle);
}

#[test]
fn a_damaged_job_payload_never_turns_hashes_on() {
    use serde_json::json;
    assert_eq!(ExportOptions::from_payload(&json!({})), off());
    assert_eq!(
        ExportOptions::from_payload(&json!({"include_hashes": "yes", "include_artwork": 1})),
        off()
    );
    assert_eq!(
        ExportOptions::from_payload(&json!({"include_hashes": true})),
        with_hashes()
    );
}

#[test]
fn the_request_body_is_strict_and_defaults_to_off() {
    let options: ExportOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(options, off());
    assert!(serde_json::from_str::<ExportOptions>(r#"{"include_hash": true}"#).is_err());
    assert!(serde_json::from_str::<ExportOptions>(r#"{"include_hashes": "true"}"#).is_err());
}

// ------------------------------------------------------------ artwork

#[test]
fn without_the_opt_in_no_image_names_a_file() {
    let s = snapshot();
    // Even with checked files on hand, the opt-in decides.
    let bundle = build(&s, off(), &named_artwork(&s)).expect("build");
    assert!(!bundle.manifest.artwork_included);
    assert!(bundle.artwork_files().is_empty());
    let images: usize = bundle.catalog.items.iter().map(|i| i.images.len()).sum();
    assert_eq!(images, 4, "provider references always travel");
}

#[test]
fn with_the_opt_in_cached_images_name_their_file_by_hash() {
    let s = snapshot();
    let bundle = build(&s, with_artwork(), &named_artwork(&s)).expect("build");
    assert!(bundle.manifest.artwork_included);
    let poster = format!("artwork/{}.jpg", sha(POSTER));
    let still = format!("artwork/{}.png", sha(STILL));
    let movie = &bundle.catalog.items[0];
    let files: Vec<_> = movie.images.iter().map(|i| i.file.as_deref()).collect();
    // Two variants share one file (the extension is lowercased); the
    // backdrop isn't downloaded yet, so it has none.
    assert_eq!(files, [Some(poster.as_str()), Some(poster.as_str()), None]);
    assert_eq!(
        bundle.artwork_files(),
        std::collections::BTreeSet::from([poster.as_str(), still.as_str()])
    );
    let bytes = zip_of(&bundle, &[(&poster, POSTER), (&still, STILL)]);
    let (mut reader, read) = BundleReader::open(Cursor::new(bytes)).expect("open");
    assert_eq!(read, bundle);
    let mut out = Vec::new();
    reader.copy_artwork(&poster, &mut out).expect("copy");
    assert_eq!(out, POSTER);
}

#[test]
fn an_image_whose_file_did_not_check_out_travels_as_a_reference() {
    let s = snapshot();
    let mut artwork = named_artwork(&s);
    artwork.retain(|(path, _), _| !path.contains("still"));
    let bundle = build(&s, with_artwork(), &artwork).expect("build");
    let still = &bundle.catalog.items[3].images[0];
    assert!(still.file.is_none());
    assert!(!still.source_url.is_empty());
}

#[test]
fn only_a_downloaded_file_with_a_hash_can_travel() {
    let h = sha(POSTER);
    assert!(cached_key(&image(MOVIE, "poster", "original", "/a/p.jpg", Some(&h))).is_some());
    assert!(cached_key(&image(MOVIE, "poster", "original", "/a/p.jpg", None)).is_none());
    assert!(cached_key(&image(MOVIE, "poster", "original", "", Some(&h))).is_none());
    assert!(cached_key(&image(
        MOVIE,
        "poster",
        "original",
        "pending:poster:original",
        Some(&h)
    ))
    .is_none());
    assert_eq!(
        artwork_name("/a/p.JPEG", &h),
        Some(format!("artwork/{h}.jpeg"))
    );
    assert_eq!(artwork_name("/a/p", &h), None, "no extension");
    assert_eq!(artwork_name("/a/p.webpx", &h), None, "extension too long");
    assert_eq!(artwork_name("/a/p.jpg", "not-a-sha"), None);
}

#[test]
fn one_image_per_kind_and_variant_and_the_cached_row_wins() {
    let h = sha(STILL);
    let stale = image(EPISODE, "still", "original", "pending:still:original", None);
    let cached = image(
        EPISODE,
        "still",
        "original",
        "/artwork/e/still-original.png",
        Some(&h),
    );
    let other = image(EPISODE, "still", "w300", "pending:still:w300", None);
    let (kept, dropped) = one_image_per_key(vec![stale.clone(), other.clone(), cached.clone()]);
    assert_eq!(
        kept.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(),
        [cached.path.as_str(), other.path.as_str()]
    );
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].path, stale.path);
    // With the duplicate, the bundle would be refused.
    let mut s = snapshot();
    s.images.push(stale);
    let refused = build(&s, off(), &HashMap::new()).unwrap().problems();
    assert!(refused
        .iter()
        .any(|p| p.message.contains("appears more than once")));
}

#[test]
fn a_cached_file_is_checked_against_its_hash() {
    let dir = scratch("verify");
    let path = dir.join("poster.jpg");
    std::fs::write(&path, POSTER).unwrap();
    assert_eq!(verify_artwork(&path, &sha(POSTER)), Ok(POSTER.len() as u64));
    assert!(verify_artwork(&path, &sha(STILL))
        .unwrap_err()
        .contains("changed"));
    assert!(verify_artwork(&dir.join("gone.jpg"), &sha(POSTER))
        .unwrap_err()
        .contains("can't be read"));
    std::fs::remove_dir_all(dir).unwrap();
}

// ------------------------------------------------------------ on disk

/// A fresh, empty directory of this test's own.
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("parlour-xfer2-{what}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_bundle_is_written_whole_or_not_at_all() {
    let dir = scratch("write");
    let cached = dir.join("cached-poster.jpg");
    std::fs::write(&cached, POSTER).unwrap();
    let s = snapshot();
    let poster = format!("artwork/{}.jpg", sha(POSTER));
    let mut artwork = named_artwork(&s);
    artwork.retain(|_, name| *name == poster);
    let bundle = build(&s, with_artwork(), &artwork).expect("build");
    let files = BTreeMap::from([(poster.clone(), cached.clone())]);

    let dest = bundle_path(&dir, id(77));
    let size = write_bundle(&dest, &bundle, &files).expect("written");
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), size);
    assert!(!dest.with_extension("zip.part").exists());
    let (reader, read) =
        BundleReader::open(std::fs::File::open(&dest).unwrap()).expect("reads back");
    assert_eq!(read, bundle);
    assert_eq!(
        reader.artwork_names().collect::<Vec<_>>(),
        [poster.as_str()]
    );

    // A file that changed after it was checked: nothing is left behind.
    std::fs::write(&cached, STILL).unwrap();
    let again = bundle_path(&dir, id(78));
    let error = write_bundle(&again, &bundle, &files).unwrap_err();
    assert!(error.contains("hash"), "{error}");
    assert!(!again.exists() && !again.with_extension("zip.part").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn downloads_are_named_by_the_export_time() {
    let at = Utc.with_ymd_and_hms(2026, 11, 24, 20, 5, 9).unwrap();
    assert_eq!(download_name(at), "parlour-export-20261124-200509.zip");
    assert_eq!(
        export_dir(std::path::Path::new("/cache")),
        PathBuf::from("/cache/exports")
    );
}

// ------------------------------------------------------------ refusals

#[test]
fn a_reference_that_leads_nowhere_fails_the_export() {
    let mut s = snapshot();
    s.access[0].profile_id = id(999);
    assert!(build(&s, off(), &HashMap::new())
        .unwrap_err()
        .contains("profile"));
    let mut s = snapshot();
    s.user_data[0].item_id = id(999);
    assert!(build(&s, off(), &HashMap::new())
        .unwrap_err()
        .contains("item"));
}

#[test]
fn a_value_the_format_does_not_know_fails_the_export() {
    let mut s = snapshot();
    s.items[0].kind = "podcast_episode".into();
    let error = build(&s, off(), &HashMap::new()).unwrap_err();
    assert!(error.contains("podcast_episode"), "{error}");
}
