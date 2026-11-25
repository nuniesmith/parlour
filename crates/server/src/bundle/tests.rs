//! XFER1: the format types against the v1 fixtures
//! (`tests/fixtures/bundle-v1/`), whole bundles through the zip and
//! back, and each way a bundle is refused.
//!
//! The fixtures are fictional throughout: people, titles and
//! `example.org` URLs are made up, IPs are RFC 5737 documentation
//! addresses, and every hash is a syntactically valid Argon2id string
//! whose salt and "hash" decode to `fixture-salt-…` /
//! `fixture-only-…` text — no password produces them.

use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};

use chrono::Utc;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use super::*;

const MANIFEST: &str = include_str!("../../tests/fixtures/bundle-v1/manifest.json");
const USERS: &str = include_str!("../../tests/fixtures/bundle-v1/users.json");
const PROFILES: &str = include_str!("../../tests/fixtures/bundle-v1/profiles.json");
const LIBRARIES: &str = include_str!("../../tests/fixtures/bundle-v1/libraries.json");
const CATALOG: &str = include_str!("../../tests/fixtures/bundle-v1/catalog.json");
const USER_DATA: &str = include_str!("../../tests/fixtures/bundle-v1/user_data.json");

/// Parse a fixture and write it back: the JSON must come back equal
/// (no field dropped or added, no value changed), and byte for byte
/// what the writer stores — so the fixtures show exactly what an
/// export looks like.
fn round_trip<T: Serialize + DeserializeOwned>(name: &str, text: &str) -> T {
    let typed: T = serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let written = serde_json::to_string_pretty(&typed).expect("serialise") + "\n";
    let original: Value = serde_json::from_str(text).expect("fixture is JSON");
    let again: Value = serde_json::from_str(&written).expect("written JSON parses");
    assert_eq!(again, original, "{name} changed on the way through");
    assert_eq!(written, text, "{name} is not what the writer writes");
    typed
}

fn fixture() -> Bundle {
    Bundle {
        manifest: round_trip(MANIFEST_JSON, MANIFEST),
        users: round_trip(USERS_JSON, USERS),
        profiles: round_trip(PROFILES_JSON, PROFILES),
        libraries: round_trip(LIBRARIES_JSON, LIBRARIES),
        catalog: round_trip(CATALOG_JSON, CATALOG),
        user_data: round_trip(USER_DATA_JSON, USER_DATA),
    }
}

fn write(bundle: &Bundle) -> Vec<u8> {
    match BundleWriter::new(Cursor::new(Vec::new()), bundle) {
        Ok(writer) => writer.finish().expect("finish").into_inner(),
        Err(e) => panic!("writer refused: {e}"),
    }
}

/// What [`BundleReader::open`] hands back for an in-memory zip.
type Opened = (BundleReader<Cursor<Vec<u8>>>, Bundle);

fn open(bytes: Vec<u8>) -> Result<Opened, BundleError> {
    BundleReader::open(Cursor::new(bytes))
}

fn refusal(bytes: Vec<u8>) -> BundleError {
    match open(bytes) {
        Ok(_) => panic!("the reader accepted it"),
        Err(e) => e,
    }
}

fn entry_text(bytes: &[u8], name: &str) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut text = String::new();
    archive
        .by_name(name)
        .expect("entry")
        .read_to_string(&mut text)
        .expect("read");
    text
}

type Entries = Vec<(String, Vec<u8>)>;

/// A zip holding exactly these entries, for the reader's refusals.
fn zip_of(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(name.as_str(), SimpleFileOptions::default())
            .expect("start");
        zip.write_all(bytes).expect("write");
    }
    zip.finish().expect("finish").into_inner()
}

/// The six documents of a bundle as zip entries, unchecked.
fn documents_of(bundle: &Bundle) -> Entries {
    vec![
        (
            MANIFEST_JSON.into(),
            serde_json::to_vec(&bundle.manifest).unwrap(),
        ),
        (
            USERS_JSON.into(),
            serde_json::to_vec(&bundle.users).unwrap(),
        ),
        (
            PROFILES_JSON.into(),
            serde_json::to_vec(&bundle.profiles).unwrap(),
        ),
        (
            LIBRARIES_JSON.into(),
            serde_json::to_vec(&bundle.libraries).unwrap(),
        ),
        (
            CATALOG_JSON.into(),
            serde_json::to_vec(&bundle.catalog).unwrap(),
        ),
        (
            USER_DATA_JSON.into(),
            serde_json::to_vec(&bundle.user_data).unwrap(),
        ),
    ]
}

fn fixture_documents() -> Entries {
    vec![
        (MANIFEST_JSON.into(), MANIFEST.as_bytes().to_vec()),
        (USERS_JSON.into(), USERS.as_bytes().to_vec()),
        (PROFILES_JSON.into(), PROFILES.as_bytes().to_vec()),
        (LIBRARIES_JSON.into(), LIBRARIES.as_bytes().to_vec()),
        (CATALOG_JSON.into(), CATALOG.as_bytes().to_vec()),
        (USER_DATA_JSON.into(), USER_DATA.as_bytes().to_vec()),
    ]
}

fn replace(mut entries: Entries, name: &str, bytes: &[u8]) -> Entries {
    let slot = entries.iter_mut().find(|(n, _)| *n == name).expect("entry");
    slot.1 = bytes.to_vec();
    entries
}

#[track_caller]
fn assert_problem(bundle: &Bundle, needle: &str) {
    let problems = bundle.problems();
    assert!(
        problems.iter().any(|p| p.to_string().contains(needle)),
        "expected a problem containing {needle:?}, got {problems:#?}"
    );
}

fn without_hashes(mut bundle: Bundle) -> Bundle {
    bundle.manifest.hashes_included = false;
    for user in &mut bundle.users.users {
        user.password_hash = None;
        user.pin_hashes = None;
    }
    bundle
}

const POSTER: &[u8] = b"fixture artwork bytes, not really a jpeg";

/// The fixture with artwork opted in and one poster carried.
fn with_artwork() -> (Bundle, String) {
    let mut bundle = fixture();
    bundle.manifest.artwork_included = true;
    let sha256 = format!("{:x}", Sha256::digest(POSTER));
    let name = artwork_file_name(&sha256, "jpg").expect("name");
    bundle.catalog.items[0].images[0].file = Some(name.clone());
    (bundle, name)
}

// ------------------------------------------------------------ the fixtures

#[test]
fn every_fixture_document_round_trips() {
    let bundle = fixture();
    assert_eq!(bundle.users.users.len(), 3);
    assert_eq!(bundle.profiles.profiles.len(), 4);
    assert_eq!(bundle.libraries.libraries.len(), 3);
    assert_eq!(bundle.catalog.items.len(), 8);
    assert_eq!(bundle.user_data.profiles.len(), 3);
    assert_eq!(bundle.user_data.activity.len(), 7);
}

#[test]
fn the_fixture_bundle_is_consistent() {
    assert_eq!(fixture().problems(), Vec::<Problem>::new());
}

#[test]
fn the_fixtures_cover_every_value_of_every_closed_set() {
    let bundle = fixture();
    let items = &bundle.catalog.items;
    let kinds: HashSet<ItemKind> = items.iter().map(|i| i.kind).collect();
    let statuses: HashSet<MetadataStatus> = items.iter().map(|i| i.metadata_status).collect();
    let segment_kinds: HashSet<SegmentKind> = items
        .iter()
        .flat_map(|i| i.segments.iter().map(|s| s.kind))
        .collect();
    let sources: HashSet<SegmentSource> = items
        .iter()
        .flat_map(|i| i.segments.iter().map(|s| s.source))
        .collect();
    let modes: HashSet<SubtitleMode> = bundle
        .profiles
        .profiles
        .iter()
        .map(|p| p.preferences.subtitle_mode)
        .collect();
    let skips: HashSet<SegmentSkip> = bundle
        .profiles
        .profiles
        .iter()
        .map(|p| p.preferences.segment_skip)
        .collect();
    assert_eq!(kinds.len(), ItemKind::ALL.len());
    assert_eq!(
        statuses.len(),
        MetadataStatus::ALL.len() - 1,
        "all but failed"
    );
    assert_eq!(segment_kinds.len(), SegmentKind::ALL.len());
    assert_eq!(sources.len(), SegmentSource::ALL.len());
    assert_eq!(modes.len(), SubtitleMode::ALL.len());
    assert_eq!(skips.len(), SegmentSkip::ALL.len());
    // A D21 marker, and an activity row whose profile belongs to
    // someone other than its user (an admin stopping a stream).
    assert!(items
        .iter()
        .flat_map(|i| &i.segments)
        .any(|s| s.start_ms.is_none() && s.source == SegmentSource::Manual));
    assert!(bundle
        .user_data
        .activity
        .iter()
        .any(|a| { matches!((&a.username, &a.profile), (Some(u), Some(p)) if *u != p.username) }));
}

// ------------------------------------------------------------ through the zip

#[test]
fn a_bundle_goes_through_the_zip_and_back() {
    let bundle = fixture();
    let (reader, back) = open(write(&bundle)).expect("opens");
    assert_eq!(back, bundle);
    assert_eq!(reader.artwork_names().count(), 0);
}

#[test]
fn the_zip_holds_the_six_documents_manifest_first_as_the_fixtures() {
    let bytes = write(&fixture());
    let mut archive = ZipArchive::new(Cursor::new(bytes.clone())).expect("zip");
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().unwrap().into_owned())
        .collect();
    assert_eq!(names, DOCUMENTS);
    for (name, text) in fixture_documents() {
        assert_eq!(entry_text(&bytes, &name).as_bytes(), text, "{name}");
    }
}

#[test]
fn without_the_confirm_no_hash_leaves_the_server() {
    let bundle = without_hashes(fixture());
    assert_eq!(bundle.problems(), Vec::<Problem>::new());
    let bytes = write(&bundle);
    for name in DOCUMENTS {
        let text = entry_text(&bytes, name);
        assert!(!text.contains("$argon2"), "{name} holds a hash");
        assert!(!text.contains("password_hash"), "{name}");
        assert!(!text.contains("pin_hashes"), "{name}");
    }
    // Which profiles have a PIN is still known.
    assert!(entry_text(&bytes, PROFILES_JSON).contains("\"has_pin\": true"));
    let (_, back) = open(bytes).expect("opens");
    assert_eq!(back, bundle);
}

#[test]
fn artwork_travels_under_its_hash() {
    let (bundle, name) = with_artwork();
    let mut writer = BundleWriter::new(Cursor::new(Vec::new()), &bundle)
        .unwrap_or_else(|e| panic!("refused: {e}"));
    let written = writer
        .add_artwork(&name, &mut &POSTER[..])
        .expect("artwork");
    assert_eq!(written, POSTER.len() as u64);
    let bytes = writer.finish().expect("finish").into_inner();

    let (mut reader, back) = open(bytes).expect("opens");
    assert_eq!(back, bundle);
    assert_eq!(
        reader.artwork_names().collect::<Vec<_>>(),
        vec![name.as_str()]
    );
    let mut out = Vec::new();
    reader.copy_artwork(&name, &mut out).expect("copy");
    assert_eq!(out, POSTER);
    let missing = artwork_file_name(&"0".repeat(64), "jpg").unwrap();
    assert!(matches!(
        reader.copy_artwork(&missing, &mut Vec::new()),
        Err(BundleError::Artwork { .. })
    ));
}

#[test]
fn the_writer_takes_only_named_artwork_whose_bytes_match() {
    let (bundle, name) = with_artwork();
    let new = || {
        BundleWriter::new(Cursor::new(Vec::new()), &bundle)
            .unwrap_or_else(|e| panic!("refused: {e}"))
    };

    let mut writer = new();
    let err = writer
        .add_artwork(&name, &mut &b"other bytes"[..])
        .expect_err("wrong bytes");
    assert!(err.to_string().contains("hash to"), "{err}");

    let mut writer = new();
    let unnamed = artwork_file_name(&"0".repeat(64), "png").unwrap();
    let err = writer
        .add_artwork(&unnamed, &mut &POSTER[..])
        .expect_err("unnamed");
    assert!(err.to_string().contains("not named"), "{err}");
    let err = writer
        .add_artwork("artwork/../poster.jpg", &mut &POSTER[..])
        .expect_err("bad name");
    assert!(err.to_string().contains("not an artwork"), "{err}");

    let mut writer = new();
    writer.add_artwork(&name, &mut &POSTER[..]).expect("first");
    let err = writer
        .add_artwork(&name, &mut &POSTER[..])
        .expect_err("twice");
    assert!(err.to_string().contains("already added"), "{err}");

    match new().finish() {
        Err(BundleError::Artwork { reason, .. }) => assert!(reason.contains("never added")),
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("finished with artwork missing"),
    }
}

#[test]
fn the_writer_refuses_an_inconsistent_bundle() {
    let mut bundle = fixture();
    let copy = bundle.catalog.items[0].clone();
    bundle.catalog.items.push(copy);
    match BundleWriter::new(Cursor::new(Vec::new()), &bundle) {
        Err(BundleError::Invalid(problems)) => assert!(problems
            .iter()
            .any(|p| p.message.contains("appears more than once"))),
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("wrote an inconsistent bundle"),
    }
}

// ------------------------------------------------------------ the reader's refusals

#[test]
fn the_reader_refuses_what_is_not_a_bundle() {
    assert!(matches!(
        refusal(b"just some text".to_vec()),
        BundleError::Zip(_)
    ));
    let no_manifest: Vec<_> = fixture_documents().into_iter().skip(1).collect();
    assert!(matches!(
        refusal(zip_of(&no_manifest)),
        BundleError::NotABundle(_)
    ));
    let other_format = replace(
        fixture_documents(),
        MANIFEST_JSON,
        br#"{"format": "someone-else", "format_version": 1}"#,
    );
    assert!(matches!(
        refusal(zip_of(&other_format)),
        BundleError::NotABundle(m) if m.contains("someone-else")
    ));
    let in_a_folder: Entries = fixture_documents()
        .into_iter()
        .map(|(n, b)| (format!("parlour-export/{n}"), b))
        .collect();
    assert!(matches!(
        refusal(zip_of(&in_a_folder)),
        BundleError::NotABundle(m) if m.contains("inside a folder")
    ));
}

#[test]
fn a_newer_version_is_named_before_its_fields_are_read() {
    let newer = replace(
        fixture_documents(),
        MANIFEST_JSON,
        br#"{"format": "parlour-export", "format_version": 2, "something_new": true}"#,
    );
    let err = refusal(zip_of(&newer));
    assert!(matches!(err, BundleError::UnsupportedVersion(2)), "{err}");
    assert_eq!(
        err.to_string(),
        "the bundle is format version 2; this server reads version 1"
    );
}

#[test]
fn a_document_larger_than_the_bound_is_refused_unread() {
    let bytes = write(&fixture());
    let catalog = entry_text(&bytes, CATALOG_JSON).len() as u64;
    // A bound just under the catalog's size: the manifest and users
    // fit, the catalog doesn't.
    let err = match BundleReader::open_with_limit(Cursor::new(bytes.clone()), catalog - 1) {
        Ok(_) => panic!("the reader accepted it"),
        Err(e) => e,
    };
    assert!(
        matches!(err, BundleError::TooLarge { file, limit } if file == CATALOG_JSON && limit == catalog - 1),
        "{err}"
    );
    assert!(BundleReader::open_with_limit(Cursor::new(bytes), catalog).is_ok());
    const { assert!(MAX_DOCUMENT_BYTES >= 64 * 1024 * 1024) };
}

#[test]
fn the_reader_refuses_missing_and_stray_entries() {
    let missing: Vec<_> = fixture_documents()
        .into_iter()
        .filter(|(n, _)| *n != CATALOG_JSON)
        .collect();
    assert!(matches!(
        refusal(zip_of(&missing)),
        BundleError::MissingEntry(CATALOG_JSON)
    ));
    let mut stray = fixture_documents();
    stray.push(("notes.txt".into(), b"hello".to_vec()));
    assert!(matches!(
        refusal(zip_of(&stray)),
        BundleError::UnexpectedEntry(n) if n == "notes.txt"
    ));
    let mut climbing = fixture_documents();
    climbing.push(("artwork/../../etc/passwd".into(), b"x".to_vec()));
    assert!(matches!(
        refusal(zip_of(&climbing)),
        BundleError::UnexpectedEntry(_)
    ));
    // A bare artwork/ folder entry is harmless.
    let mut folder = fixture_documents();
    folder.push((ARTWORK_DIR.into(), Vec::new()));
    assert!(open(zip_of(&folder)).is_ok());
}

#[test]
fn the_reader_refuses_unknown_fields() {
    let mut profiles: Value = serde_json::from_str(PROFILES).unwrap();
    profiles["profiles"][0]["favourite_colour"] = Value::from("green");
    let entries = replace(
        fixture_documents(),
        PROFILES_JSON,
        &serde_json::to_vec(&profiles).unwrap(),
    );
    match refusal(zip_of(&entries)) {
        BundleError::Json { file, source } => {
            assert_eq!(file, PROFILES_JSON);
            assert!(source.to_string().contains("favourite_colour"), "{source}");
        }
        e => panic!("wrong error: {e}"),
    }
}

#[test]
fn the_reader_refuses_an_inconsistent_bundle() {
    let mut bundle = fixture();
    bundle.user_data.profiles[0].items[0].item.stable_key = "movie:gone:0".into();
    match refusal(zip_of(&documents_of(&bundle))) {
        BundleError::Invalid(problems) => assert!(problems
            .iter()
            .any(|p| p.file == USER_DATA_JSON && p.message.contains("movie:gone:0"))),
        e => panic!("wrong error: {e}"),
    }
}

#[test]
fn the_reader_holds_artwork_to_the_catalog() {
    let (bundle, name) = with_artwork();
    // Named by the catalog, absent from the zip.
    match refusal(zip_of(&documents_of(&bundle))) {
        BundleError::Invalid(problems) => assert!(problems
            .iter()
            .any(|p| p.message.contains("is not in the zip"))),
        e => panic!("wrong error: {e}"),
    }
    // In the zip, named by nothing.
    let mut entries = documents_of(&fixture());
    let stray = artwork_file_name(&"ab".repeat(32), "png").unwrap();
    entries.push((stray, POSTER.to_vec()));
    match refusal(zip_of(&entries)) {
        BundleError::Invalid(problems) => assert!(problems
            .iter()
            .any(|p| p.file == ARTWORK_DIR && p.message.contains("not named"))),
        e => panic!("wrong error: {e}"),
    }
    // Present but altered: opens (names agree), copying catches it.
    let mut entries = documents_of(&bundle);
    entries.push((name.clone(), b"altered bytes".to_vec()));
    let (mut reader, _) = open(zip_of(&entries)).expect("opens");
    let err = reader
        .copy_artwork(&name, &mut Vec::new())
        .expect_err("altered");
    assert!(err.to_string().contains("hash to"), "{err}");
}

// ------------------------------------------------------------ the checks

#[test]
fn hashes_are_all_or_nothing() {
    let mut bundle = fixture();
    bundle.users.users[1].password_hash = None;
    assert_problem(&bundle, "user 'ben' has no password hash");

    let mut bundle = without_hashes(fixture());
    bundle.users.users[0].password_hash = fixture().users.users[0].password_hash.clone();
    assert_problem(&bundle, "user 'ada' carries a password hash");

    let mut bundle = without_hashes(fixture());
    bundle.users.users[0].pin_hashes = Some(BTreeMap::new());
    assert_problem(&bundle, "user 'ada' carries PIN hashes");

    let mut bundle = fixture();
    bundle.users.users[2].pin_hashes = None;
    assert_problem(&bundle, "user 'cora' has no pin_hashes");

    let mut bundle = fixture();
    bundle.users.users[0].pin_hashes = Some(BTreeMap::new());
    assert_problem(
        &bundle,
        "profile 'Kids' of 'ada' has a PIN, but no PIN hash",
    );

    let pin = fixture().users.users[0].pin_hashes.as_ref().unwrap()["Kids"].clone();
    let mut bundle = fixture();
    let pins = bundle.users.users[1].pin_hashes.as_mut().unwrap();
    pins.insert("Ben".into(), pin.clone());
    pins.insert("Ada".into(), pin);
    assert_problem(&bundle, "PIN hash for 'Ben', whose has_pin is false");
    assert_problem(
        &bundle,
        "PIN hash for 'Ada', which is not one of their profiles",
    );
}

#[test]
fn a_hash_is_an_argon2id_phc_string() {
    let mut bundle = fixture();
    bundle.users.users[0].password_hash = Some("plain text".into());
    assert_problem(&bundle, "the password hash is not a PHC hash string");
    let argon2i = fixture().users.users[0]
        .password_hash
        .clone()
        .unwrap()
        .replacen("$argon2id$", "$argon2i$", 1);
    bundle.users.users[0].password_hash = Some(argon2i);
    assert_problem(&bundle, "is not an Argon2id hash");
    // What the server really stores is accepted.
    let real = crate::auth::hash_password("fixture password, used nowhere").expect("hash");
    bundle.users.users[0].password_hash = Some(real);
    assert_eq!(bundle.problems(), Vec::<Problem>::new());
}

#[test]
fn keys_are_unique() {
    let mut bundle = fixture();
    bundle.users.users[1].username = "ADA".into();
    assert_problem(&bundle, "username 'ADA' appears more than once");

    let mut bundle = fixture();
    let copy = bundle.profiles.profiles[1].clone();
    bundle.profiles.profiles.push(copy);
    assert_problem(&bundle, "profile 'Kids' of 'ada' appears more than once");

    let mut bundle = fixture();
    let searches = &mut bundle.profiles.profiles[0].recent_searches;
    let mut again = searches[0].clone();
    again.query = again.query.to_uppercase();
    searches.push(again);
    assert_problem(
        &bundle,
        "recent search 'NIGHT HARBOR' appears more than once",
    );

    let mut bundle = fixture();
    bundle.libraries.libraries[1].name = "Films".into();
    assert_problem(&bundle, "library 'Films' appears more than once");

    let mut bundle = fixture();
    bundle.libraries.libraries[1]
        .paths
        .push("/media/series".into());
    assert_problem(&bundle, "path '/media/series' appears more than once");

    let mut bundle = fixture();
    let grant = bundle.libraries.libraries[0].access[1].clone();
    bundle.libraries.libraries[0].access.push(grant);
    assert_problem(&bundle, "has more than one access grant");

    let mut bundle = fixture();
    let copy = bundle.catalog.items[3].clone();
    bundle.catalog.items.push(copy);
    assert_problem(
        &bundle,
        "item 'series:night-harbor:2019' in 'Series' appears more than once",
    );

    let mut bundle = fixture();
    let credit = bundle.catalog.items[0].people[0].clone();
    bundle.catalog.items[0].people.push(credit);
    assert_problem(
        &bundle,
        "'Juniper Hale' is credited as actor more than once",
    );

    let mut bundle = fixture();
    let segment = bundle.catalog.items[0].segments[0].clone();
    bundle.catalog.items[0].segments.push(segment);
    assert_problem(
        &bundle,
        "the credits segment from chapters appears more than once",
    );

    let mut bundle = fixture();
    let image = bundle.catalog.items[0].images[1].clone();
    bundle.catalog.items[0].images.push(image);
    assert_problem(&bundle, "the poster image 'w300' appears more than once");

    let mut bundle = fixture();
    let group = bundle.user_data.profiles[1].clone();
    bundle.user_data.profiles.push(group);
    assert_problem(&bundle, "profile 'Kids' of 'ada' has more than one group");

    let mut bundle = fixture();
    let row = bundle.user_data.profiles[0].items[0].clone();
    bundle.user_data.profiles[0].items.push(row);
    assert_problem(
        &bundle,
        "item 'series:night-harbor:2019:s01:e001' in 'Series' appears more than once",
    );
}

#[test]
fn references_between_documents_lead_somewhere() {
    let mut bundle = fixture();
    bundle.profiles.profiles[3].username = "dora".into();
    assert_problem(&bundle, "profile 'Cora' of 'dora': no such user");

    let mut bundle = fixture();
    bundle.libraries.libraries[0].access[0].profile = "Nobody".into();
    assert_problem(&bundle, "access for profile 'Nobody' of 'ada'");

    let mut bundle = fixture();
    bundle.catalog.items[2].library = "Shorts".into();
    assert_problem(&bundle, "no such library");

    let mut bundle = fixture();
    bundle.user_data.profiles[2].profile = "Benjamin".into();
    assert_problem(&bundle, "profile 'Benjamin' of 'ben': not in profiles.json");

    let mut bundle = fixture();
    bundle.user_data.activity[0].username = Some("zed".into());
    bundle.user_data.activity[2].profile = Some(ProfileRef {
        username: "ada".into(),
        profile: "Guest".into(),
    });
    bundle.user_data.activity[4].item = Some(ItemRef {
        library: "Series".into(),
        stable_key: "series:night-harbor:2019:s09:e001".into(),
    });
    assert_problem(&bundle, "activity entry 1 (auth.login): user 'zed'");
    assert_problem(
        &bundle,
        "activity entry 3 (import.completed): profile 'Guest'",
    );
    assert_problem(
        &bundle,
        "activity entry 5 (playback.start): item 'series:night-harbor:2019:s09:e001'",
    );
}

fn segment(bundle: &mut Bundle) -> &mut Segment {
    &mut bundle.catalog.items[0].segments[0]
}

#[test]
fn segments_follow_the_database_checks() {
    let mut bundle = fixture();
    let s = segment(&mut bundle);
    s.start_ms = None;
    s.end_ms = None;
    assert_problem(&bundle, "has no times (only a manual row may be a marker)");

    let mut bundle = fixture();
    segment(&mut bundle).end_ms = None;
    assert_problem(&bundle, "has only one of start_ms and end_ms");

    let mut bundle = fixture();
    let s = segment(&mut bundle);
    s.end_ms = s.start_ms;
    assert_problem(&bundle, "does not end after it starts");

    let mut bundle = fixture();
    segment(&mut bundle).start_ms = Some(-1);
    assert_problem(&bundle, "starts before 0");

    let mut bundle = fixture();
    segment(&mut bundle).confidence = 1.5;
    assert_problem(&bundle, "has confidence 1.5 (must be 0–1)");
}

#[test]
fn parents_stay_in_their_library_and_never_loop() {
    let mut bundle = fixture();
    bundle.catalog.items[4].parent = Some("series:elsewhere:2019".into());
    assert_problem(
        &bundle,
        "its parent 'series:elsewhere:2019' is not an item in the same library",
    );

    let mut bundle = fixture();
    bundle.catalog.items[0].parent = Some("series:night-harbor:2019".into());
    assert_problem(&bundle, "is not an item in the same library");

    let mut bundle = fixture();
    bundle.catalog.items[3].parent = Some("series:night-harbor:2019:s01:e001".into());
    assert_problem(&bundle, "its parent chain loops");
}

#[test]
fn a_person_has_one_tmdb_id() {
    let mut bundle = fixture();
    bundle.catalog.items[3].people[0].tmdb_id = Some(1);
    assert_problem(&bundle, "person 'Juniper Hale' has a different tmdb_id");
}

#[test]
fn artwork_needs_the_opt_in_and_a_proper_name() {
    let (mut bundle, _) = with_artwork();
    assert_eq!(bundle.problems(), Vec::<Problem>::new());
    bundle.manifest.artwork_included = false;
    assert_problem(&bundle, "but the manifest says artwork is not included");

    let (mut bundle, _) = with_artwork();
    bundle.catalog.items[0].images[0].file = Some("artwork/../../poster.jpg".into());
    assert_problem(&bundle, "which is not an artwork/<sha256>.<ext> name");
}

#[test]
fn artwork_names_are_strict() {
    let sha = "0123456789abcdef".repeat(4);
    assert_eq!(
        parse_artwork_name(&format!("artwork/{sha}.jpg")),
        Some((sha.as_str(), "jpg"))
    );
    assert_eq!(
        artwork_file_name(&sha, "webp").as_deref(),
        Some(format!("artwork/{sha}.webp").as_str())
    );
    for bad in [
        format!("artwork/{sha}"),
        format!("artwork/{sha}.JPG"),
        format!("artwork/{sha}.jpeg5"),
        format!("artwork/{sha}.j.p"),
        format!("artwork/sub/{sha}.jpg"),
        format!("{sha}.jpg"),
        format!("artwork/{}.jpg", sha.to_uppercase()),
        format!("artwork/{}.jpg", &sha[1..]),
        "artwork/../x.jpg".to_string(),
    ] {
        assert_eq!(parse_artwork_name(&bad), None, "{bad}");
    }
    assert_eq!(artwork_file_name("not-hex", "jpg"), None);
}

// ------------------------------------------------------------ the format's own rules

#[test]
fn a_new_manifest_names_this_format_and_server() {
    let manifest = Manifest::new(Utc::now(), false, true);
    assert_eq!(manifest.format, FORMAT_NAME);
    assert_eq!(manifest.format_version, FORMAT_VERSION);
    assert_eq!(manifest.server_version, env!("CARGO_PKG_VERSION"));
    assert!(!manifest.hashes_included);
    assert!(manifest.artwork_included);
}

macro_rules! assert_wire_names {
    ($($enum:ident),+) => {$(
        for value in $enum::ALL {
            assert_eq!($enum::parse(value.as_str()), Some(*value));
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                Value::from(value.as_str()),
                "{} {:?}", stringify!($enum), value
            );
        }
        assert_eq!($enum::parse("not-a-value"), None);
    )+};
}

#[test]
fn closed_sets_serialise_as_their_database_names() {
    assert_wire_names!(
        LibraryKind,
        ItemKind,
        MetadataStatus,
        SegmentKind,
        SegmentSource,
        ImageKind,
        SubtitleMode,
        SegmentSkip
    );
}

#[test]
fn closed_sets_match_the_rest_of_the_server() {
    use parlour_core::SegmentKind as CoreKind;
    use parlour_core::SegmentSource as CoreSource;
    for core in [CoreKind::Recap, CoreKind::Intro, CoreKind::Credits] {
        assert_eq!(SegmentKind::from(core).as_str(), core.as_str());
        assert_eq!(CoreKind::from(SegmentKind::from(core)), core);
    }
    assert_eq!(SegmentKind::ALL.len(), 3);
    for core in [
        CoreSource::Chapters,
        CoreSource::Fingerprint,
        CoreSource::Manual,
        CoreSource::Heuristic,
    ] {
        assert_eq!(SegmentSource::from(core).as_str(), core.as_str());
        assert_eq!(CoreSource::from(SegmentSource::from(core)), core);
    }
    assert_eq!(SegmentSource::ALL.len(), 4);
    for kind in LibraryKind::ALL {
        assert!(parlour_catalog::kind_from_str(kind.as_str()).is_some());
    }
    let names = |all: &[&'static str]| all.to_vec();
    assert_eq!(
        SubtitleMode::ALL
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>(),
        names(crate::profiles::SUBTITLE_MODES)
    );
    assert_eq!(
        SegmentSkip::ALL
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>(),
        names(crate::profiles::SEGMENT_SKIP_MODES)
    );
}

#[test]
fn the_coverage_list_names_each_table_and_column_once() {
    let mut tables = HashSet::new();
    for table in coverage::SCHEMA {
        assert!(tables.insert(table.name()), "{} listed twice", table.name());
        if let coverage::Table::Carried { name, columns } = table {
            let mut seen = HashSet::new();
            for (column, _) in *columns {
                assert!(seen.insert(*column), "{name}.{column} listed twice");
            }
            assert!(
                columns
                    .iter()
                    .any(|(_, how)| matches!(how, coverage::Column::Value(_))),
                "{name} is carried but carries no value"
            );
        }
    }
    for (table, _, values) in coverage::check_constraints() {
        assert!(tables.contains(table), "{table}");
        assert!(!values.is_empty());
    }
}
