//! The consistency checks behind [`Bundle::problems`]: the rules the
//! database itself would enforce (unique keys, CHECK constraints) and
//! the references between documents. Server policy that the database
//! doesn't enforce (name lengths, the ten-search limit, …) is not a
//! format rule and isn't checked here.

use std::collections::{HashMap, HashSet};
use std::fmt;

use argon2::password_hash::PasswordHash;

use super::{
    parse_artwork_name, Bundle, Manifest, Profile, SegmentSource, CATALOG_JSON, FORMAT_NAME,
    FORMAT_VERSION, LIBRARIES_JSON, MANIFEST_JSON, PROFILES_JSON, USERS_JSON, USER_DATA_JSON,
};

/// One thing wrong with a bundle, located by document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The document it is in (`users.json`, …), or `artwork/`.
    pub file: &'static str,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

#[derive(Default)]
struct Problems(Vec<Problem>);

impl Problems {
    fn add(&mut self, file: &'static str, message: impl Into<String>) {
        self.0.push(Problem {
            file,
            message: message.into(),
        });
    }
}

type ProfileKey<'a> = (&'a str, &'a str);
type ItemKey<'a> = (&'a str, &'a str);

pub(super) fn problems(bundle: &Bundle) -> Vec<Problem> {
    let mut p = Problems::default();
    manifest(&bundle.manifest, &mut p);
    let users = users(bundle, &mut p);
    let profiles = profiles(bundle, &users, &mut p);
    credentials(bundle, &profiles, &mut p);
    let libraries = libraries(bundle, &profiles, &mut p);
    let items = catalog(bundle, &libraries, &mut p);
    user_data(bundle, &users, &profiles, &items, &mut p);
    p.0
}

fn manifest(manifest: &Manifest, p: &mut Problems) {
    if manifest.format != FORMAT_NAME {
        p.add(
            MANIFEST_JSON,
            format!("format is '{}', not '{FORMAT_NAME}'", manifest.format),
        );
    }
    if manifest.format_version != FORMAT_VERSION {
        p.add(
            MANIFEST_JSON,
            format!(
                "format_version is {}, not {FORMAT_VERSION}",
                manifest.format_version
            ),
        );
    }
}

/// Usernames are unique ignoring case (`users_username_lower_idx`);
/// references elsewhere must match a username exactly.
fn users<'a>(bundle: &'a Bundle, p: &mut Problems) -> HashSet<&'a str> {
    let mut exact = HashSet::new();
    let mut folded = HashSet::new();
    for user in &bundle.users.users {
        if user.username.trim().is_empty() {
            p.add(USERS_JSON, "a user has an empty username");
        }
        if !folded.insert(user.username.to_lowercase()) {
            p.add(
                USERS_JSON,
                format!(
                    "username '{}' appears more than once (usernames are unique ignoring case)",
                    user.username
                ),
            );
        }
        exact.insert(user.username.as_str());
    }
    exact
}

fn profiles<'a>(
    bundle: &'a Bundle,
    users: &HashSet<&str>,
    p: &mut Problems,
) -> HashMap<ProfileKey<'a>, &'a Profile> {
    let mut out = HashMap::new();
    for profile in &bundle.profiles.profiles {
        let who = format!("profile '{}' of '{}'", profile.name, profile.username);
        if !users.contains(profile.username.as_str()) {
            p.add(
                PROFILES_JSON,
                format!("{who}: no such user in {USERS_JSON}"),
            );
        }
        if profile.name.trim().is_empty() {
            p.add(
                PROFILES_JSON,
                format!(
                    "user '{}' has a profile with an empty name",
                    profile.username
                ),
            );
        }
        if out
            .insert((profile.username.as_str(), profile.name.as_str()), profile)
            .is_some()
        {
            p.add(PROFILES_JSON, format!("{who} appears more than once"));
        }
        let mut queries = HashSet::new();
        for search in &profile.recent_searches {
            if !queries.insert(search.query.to_lowercase()) {
                p.add(
                    PROFILES_JSON,
                    format!(
                        "{who}: recent search '{}' appears more than once (ignoring case)",
                        search.query
                    ),
                );
            }
        }
    }
    out
}

/// A hash must be what Parlour stores: an Argon2id PHC string.
fn check_hash(hash: &str) -> Result<(), &'static str> {
    match PasswordHash::new(hash) {
        Ok(parsed) if parsed.algorithm.as_str() == "argon2id" => Ok(()),
        Ok(_) => Err("is not an Argon2id hash"),
        Err(_) => Err("is not a PHC hash string"),
    }
}

/// Hashes are all or nothing: the manifest says whether the admin
/// confirmed them, and then every login and every PIN has its hash.
fn credentials(bundle: &Bundle, profiles: &HashMap<ProfileKey<'_>, &Profile>, p: &mut Problems) {
    let included = bundle.manifest.hashes_included;
    for user in &bundle.users.users {
        let name = &user.username;
        match (&user.password_hash, included) {
            (Some(_), false) => p.add(
                USERS_JSON,
                format!("user '{name}' carries a password hash, but the manifest says hashes are not included"),
            ),
            (None, true) => p.add(
                USERS_JSON,
                format!("user '{name}' has no password hash, but the manifest says hashes are included"),
            ),
            (Some(hash), true) => {
                if let Err(why) = check_hash(hash) {
                    p.add(USERS_JSON, format!("user '{name}': the password hash {why}"));
                }
            }
            (None, false) => {}
        }
        match (&user.pin_hashes, included) {
            (Some(_), false) => p.add(
                USERS_JSON,
                format!("user '{name}' carries PIN hashes, but the manifest says hashes are not included"),
            ),
            (None, true) => p.add(
                USERS_JSON,
                format!("user '{name}' has no pin_hashes, but the manifest says hashes are included"),
            ),
            (Some(pins), true) => {
                for (profile, hash) in pins {
                    match profiles.get(&(name.as_str(), profile.as_str())) {
                        None => p.add(
                            USERS_JSON,
                            format!("user '{name}' has a PIN hash for '{profile}', which is not one of their profiles"),
                        ),
                        Some(found) if !found.has_pin => p.add(
                            USERS_JSON,
                            format!("user '{name}' has a PIN hash for '{profile}', whose has_pin is false"),
                        ),
                        Some(_) => {}
                    }
                    if let Err(why) = check_hash(hash) {
                        p.add(
                            USERS_JSON,
                            format!("user '{name}': the PIN hash for '{profile}' {why}"),
                        );
                    }
                }
            }
            (None, false) => {}
        }
    }
    if included {
        for profile in &bundle.profiles.profiles {
            if !profile.has_pin {
                continue;
            }
            let has_hash = bundle
                .users
                .users
                .iter()
                .find(|u| u.username == profile.username)
                .and_then(|u| u.pin_hashes.as_ref())
                .is_some_and(|pins| pins.contains_key(&profile.name));
            if !has_hash {
                p.add(
                    USERS_JSON,
                    format!(
                        "profile '{}' of '{}' has a PIN, but no PIN hash is included",
                        profile.name, profile.username
                    ),
                );
            }
        }
    }
}

fn libraries<'a>(
    bundle: &'a Bundle,
    profiles: &HashMap<ProfileKey<'_>, &Profile>,
    p: &mut Problems,
) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    for library in &bundle.libraries.libraries {
        let name = &library.name;
        if name.trim().is_empty() {
            p.add(LIBRARIES_JSON, "a library has an empty name");
        }
        if !names.insert(name.as_str()) {
            p.add(
                LIBRARIES_JSON,
                format!("library '{name}' appears more than once"),
            );
        }
        let mut paths = HashSet::new();
        for path in &library.paths {
            if !paths.insert(path.as_str()) {
                p.add(
                    LIBRARIES_JSON,
                    format!("library '{name}': path '{path}' appears more than once"),
                );
            }
        }
        let mut grants = HashSet::new();
        for grant in &library.access {
            let key = (grant.username.as_str(), grant.profile.as_str());
            if !profiles.contains_key(&key) {
                p.add(
                    LIBRARIES_JSON,
                    format!(
                        "library '{name}': access for profile '{}' of '{}', which is not in {PROFILES_JSON}",
                        grant.profile, grant.username
                    ),
                );
            }
            if !grants.insert(key) {
                p.add(
                    LIBRARIES_JSON,
                    format!(
                        "library '{name}': profile '{}' of '{}' has more than one access grant",
                        grant.profile, grant.username
                    ),
                );
            }
        }
    }
    names
}

fn catalog<'a>(
    bundle: &'a Bundle,
    libraries: &HashSet<&str>,
    p: &mut Problems,
) -> HashSet<ItemKey<'a>> {
    let artwork_included = bundle.manifest.artwork_included;
    let mut keys: HashMap<ItemKey<'a>, Option<&'a str>> = HashMap::new();
    // People are one row per name server-wide, so a name carries one
    // tmdb_id wherever it is credited.
    let mut people: HashMap<&str, Option<i64>> = HashMap::new();
    for item in &bundle.catalog.items {
        let at = format!("item '{}' in '{}'", item.stable_key, item.library);
        if !libraries.contains(item.library.as_str()) {
            p.add(
                CATALOG_JSON,
                format!("{at}: no such library in {LIBRARIES_JSON}"),
            );
        }
        if item.stable_key.is_empty() {
            p.add(
                CATALOG_JSON,
                format!("an item in '{}' has an empty stable_key", item.library),
            );
        }
        if keys
            .insert(
                (item.library.as_str(), item.stable_key.as_str()),
                item.parent.as_deref(),
            )
            .is_some()
        {
            p.add(CATALOG_JSON, format!("{at} appears more than once"));
        }

        let mut credits = HashSet::new();
        for credit in &item.people {
            if !credits.insert((credit.name.as_str(), credit.role.as_str())) {
                p.add(
                    CATALOG_JSON,
                    format!(
                        "{at}: '{}' is credited as {} more than once",
                        credit.name, credit.role
                    ),
                );
            }
            match people.get(credit.name.as_str()) {
                Some(seen) if *seen != credit.tmdb_id => p.add(
                    CATALOG_JSON,
                    format!(
                        "{at}: person '{}' has a different tmdb_id than elsewhere in the bundle (people are unique by name)",
                        credit.name
                    ),
                ),
                Some(_) => {}
                None => {
                    people.insert(credit.name.as_str(), credit.tmdb_id);
                }
            }
        }

        let mut segments = HashSet::new();
        for segment in &item.segments {
            let what = format!(
                "{at}: the {} segment from {}",
                segment.kind.as_str(),
                segment.source.as_str()
            );
            if !segments.insert((segment.kind, segment.source)) {
                p.add(CATALOG_JSON, format!("{what} appears more than once"));
            }
            match (segment.start_ms, segment.end_ms) {
                (None, None) => {
                    if segment.source != SegmentSource::Manual {
                        p.add(
                            CATALOG_JSON,
                            format!("{what} has no times (only a manual row may be a marker)"),
                        );
                    }
                }
                (Some(start), Some(end)) => {
                    if start < 0 {
                        p.add(CATALOG_JSON, format!("{what} starts before 0"));
                    }
                    if end <= start {
                        p.add(CATALOG_JSON, format!("{what} does not end after it starts"));
                    }
                }
                _ => p.add(
                    CATALOG_JSON,
                    format!("{what} has only one of start_ms and end_ms"),
                ),
            }
            if !(0.0..=1.0).contains(&segment.confidence) {
                p.add(
                    CATALOG_JSON,
                    format!("{what} has confidence {} (must be 0–1)", segment.confidence),
                );
            }
        }

        let mut images = HashSet::new();
        for image in &item.images {
            let what = format!(
                "{at}: the {} image '{}'",
                image.kind.as_str(),
                image.variant
            );
            if !images.insert((image.kind, image.variant.as_str())) {
                p.add(CATALOG_JSON, format!("{what} appears more than once"));
            }
            if image.source_url.is_empty() {
                p.add(CATALOG_JSON, format!("{what} has an empty source_url"));
            }
            if let Some(file) = &image.file {
                if !artwork_included {
                    p.add(
                        CATALOG_JSON,
                        format!("{what} names the artwork file '{file}', but the manifest says artwork is not included"),
                    );
                } else if parse_artwork_name(file).is_none() {
                    p.add(
                        CATALOG_JSON,
                        format!(
                            "{what} names '{file}', which is not an artwork/<sha256>.<ext> name"
                        ),
                    );
                }
            }
        }
    }

    // Parents: in the same library, and no chain that loops.
    for item in &bundle.catalog.items {
        let Some(parent) = item.parent.as_deref() else {
            continue;
        };
        let at = format!("item '{}' in '{}'", item.stable_key, item.library);
        let library = item.library.as_str();
        if !keys.contains_key(&(library, parent)) {
            p.add(
                CATALOG_JSON,
                format!("{at}: its parent '{parent}' is not an item in the same library"),
            );
            continue;
        }
        let mut seen = HashSet::from([item.stable_key.as_str()]);
        let mut cursor = Some(parent);
        while let Some(key) = cursor {
            if !seen.insert(key) {
                p.add(CATALOG_JSON, format!("{at}: its parent chain loops"));
                break;
            }
            cursor = keys.get(&(library, key)).copied().flatten();
        }
    }
    keys.into_keys().collect()
}

fn user_data(
    bundle: &Bundle,
    users: &HashSet<&str>,
    profiles: &HashMap<ProfileKey<'_>, &Profile>,
    items: &HashSet<ItemKey<'_>>,
    p: &mut Problems,
) {
    let mut groups = HashSet::new();
    for group in &bundle.user_data.profiles {
        let who = format!("profile '{}' of '{}'", group.profile, group.username);
        let key = (group.username.as_str(), group.profile.as_str());
        if !profiles.contains_key(&key) {
            p.add(USER_DATA_JSON, format!("{who}: not in {PROFILES_JSON}"));
        }
        if !groups.insert(key) {
            p.add(USER_DATA_JSON, format!("{who} has more than one group"));
        }
        let mut seen = HashSet::new();
        for row in &group.items {
            let item = (row.item.library.as_str(), row.item.stable_key.as_str());
            if !items.contains(&item) {
                p.add(
                    USER_DATA_JSON,
                    format!(
                        "{who}: item '{}' in '{}' is not in {CATALOG_JSON}",
                        item.1, item.0
                    ),
                );
            }
            if !seen.insert(item) {
                p.add(
                    USER_DATA_JSON,
                    format!(
                        "{who}: item '{}' in '{}' appears more than once",
                        item.1, item.0
                    ),
                );
            }
        }
    }
    for (index, entry) in bundle.user_data.activity.iter().enumerate() {
        let at = format!("activity entry {} ({})", index + 1, entry.kind);
        if let Some(username) = &entry.username {
            if !users.contains(username.as_str()) {
                p.add(
                    USER_DATA_JSON,
                    format!("{at}: user '{username}' is not in {USERS_JSON}"),
                );
            }
        }
        if let Some(profile) = &entry.profile {
            if !profiles.contains_key(&(profile.username.as_str(), profile.profile.as_str())) {
                p.add(
                    USER_DATA_JSON,
                    format!(
                        "{at}: profile '{}' of '{}' is not in {PROFILES_JSON}",
                        profile.profile, profile.username
                    ),
                );
            }
        }
        if let Some(item) = &entry.item {
            if !items.contains(&(item.library.as_str(), item.stable_key.as_str())) {
                p.add(
                    USER_DATA_JSON,
                    format!(
                        "{at}: item '{}' in '{}' is not in {CATALOG_JSON}",
                        item.stable_key, item.library
                    ),
                );
            }
        }
    }
}
