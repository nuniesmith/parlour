//! The zip layout. [`BundleWriter`] stores a bundle and
//! [`BundleReader`] opens one; both refuse anything that isn't a
//! consistent `parlour-export` v1 bundle, so a bundle that opens is
//! one XFER3 can apply as it stands.

use std::collections::BTreeSet;
use std::io::{self, BufReader, BufWriter, Read, Seek, Write};

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::{
    Bundle, BundleError, Problem, ARTWORK_DIR, CATALOG_JSON, DOCUMENTS, FORMAT_NAME,
    FORMAT_VERSION, LIBRARIES_JSON, MANIFEST_JSON, PROFILES_JSON, USERS_JSON, USER_DATA_JSON,
};

/// The zip entry name of an artwork file, `artwork/<sha256>.<ext>`:
/// `sha256` is the lowercase hex SHA-256 of the file's bytes (what
/// `images.hash` holds) and `ext` the cached file's extension. `None`
/// if either part doesn't fit the pattern [`parse_artwork_name`]
/// accepts.
pub fn artwork_file_name(sha256: &str, ext: &str) -> Option<String> {
    let name = format!("{ARTWORK_DIR}{sha256}.{ext}");
    parse_artwork_name(&name).is_some().then_some(name)
}

/// `(sha256, ext)` of an artwork entry name, or `None` unless it is
/// exactly `artwork/` + 64 lowercase hex digits + `.` + 1–4 lowercase
/// letters or digits. Nothing else is allowed under `artwork/`, so a
/// name can never climb out of the folder it is extracted into.
pub fn parse_artwork_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(ARTWORK_DIR)?;
    let (sha256, ext) = rest.split_once('.')?;
    let hex = sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let ext_ok = (1..=4).contains(&ext.len())
        && ext
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
    (hex && ext_ok).then_some((sha256, ext))
}

/// Entry times: the export time, at the 2 s resolution zip allows.
fn zip_time(at: DateTime<Utc>) -> zip::DateTime {
    u16::try_from(at.year())
        .ok()
        .and_then(|year| {
            zip::DateTime::from_date_and_time(
                year,
                at.month() as u8,
                at.day() as u8,
                at.hour() as u8,
                at.minute() as u8,
                at.second() as u8,
            )
            .ok()
        })
        .unwrap_or_default()
}

/// Copies through to `inner` while hashing what passed.
struct HashingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    hasher: Sha256,
}

impl<W: Write + ?Sized> Write for HashingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn artwork_error(name: &str, reason: impl Into<String>) -> BundleError {
    BundleError::Artwork {
        name: name.to_string(),
        reason: reason.into(),
    }
}

fn put_json<W: Write + Seek, T: Serialize>(
    zip: &mut ZipWriter<W>,
    options: SimpleFileOptions,
    name: &'static str,
    value: &T,
) -> Result<(), BundleError> {
    zip.start_file(name, options)?;
    let mut out = BufWriter::new(&mut *zip);
    serde_json::to_writer_pretty(&mut out, value)
        .map_err(|source| BundleError::Json { file: name, source })?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// Writes one bundle: the six documents first (manifest first), then
/// whatever artwork the catalog names, then [`BundleWriter::finish`].
pub struct BundleWriter<W: Write + Seek> {
    zip: ZipWriter<W>,
    time: zip::DateTime,
    wanted: BTreeSet<String>,
    added: BTreeSet<String>,
}

impl<W: Write + Seek> BundleWriter<W> {
    /// Check the bundle and write its documents. An inconsistent
    /// bundle is refused before anything is written.
    pub fn new(writer: W, bundle: &Bundle) -> Result<Self, BundleError> {
        let problems = bundle.problems();
        if !problems.is_empty() {
            return Err(BundleError::Invalid(problems));
        }
        let time = zip_time(bundle.manifest.exported_at);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(time);
        let mut zip = ZipWriter::new(writer);
        put_json(&mut zip, options, MANIFEST_JSON, &bundle.manifest)?;
        put_json(&mut zip, options, USERS_JSON, &bundle.users)?;
        put_json(&mut zip, options, PROFILES_JSON, &bundle.profiles)?;
        put_json(&mut zip, options, LIBRARIES_JSON, &bundle.libraries)?;
        put_json(&mut zip, options, CATALOG_JSON, &bundle.catalog)?;
        put_json(&mut zip, options, USER_DATA_JSON, &bundle.user_data)?;
        Ok(Self {
            zip,
            time,
            wanted: bundle
                .artwork_files()
                .into_iter()
                .map(str::to_string)
                .collect(),
            added: BTreeSet::new(),
        })
    }

    /// Add one artwork file the catalog names. Its bytes must hash to
    /// the name; images are stored as they are, not deflated again.
    /// On an error the zip is unusable — discard it.
    pub fn add_artwork<R: Read + ?Sized>(
        &mut self,
        name: &str,
        content: &mut R,
    ) -> Result<u64, BundleError> {
        let Some((sha256, _)) = parse_artwork_name(name) else {
            return Err(artwork_error(name, "is not an artwork/<sha256>.<ext> name"));
        };
        if !self.wanted.contains(name) {
            return Err(artwork_error(name, "is not named by catalog.json"));
        }
        if self.added.contains(name) {
            return Err(artwork_error(name, "was already added"));
        }
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(self.time);
        self.zip.start_file(name, options)?;
        let mut out = HashingWriter {
            inner: &mut self.zip,
            hasher: Sha256::new(),
        };
        let written = io::copy(content, &mut out)?;
        let digest = format!("{:x}", out.hasher.finalize());
        if digest != sha256 {
            return Err(artwork_error(name, format!("its bytes hash to {digest}")));
        }
        self.added.insert(name.to_string());
        Ok(written)
    }

    /// Finish the zip. Every artwork file the catalog names must have
    /// been added.
    pub fn finish(self) -> Result<W, BundleError> {
        if let Some(missing) = self.wanted.difference(&self.added).next() {
            return Err(artwork_error(
                missing,
                "is named by catalog.json but was never added",
            ));
        }
        Ok(self.zip.finish()?)
    }
}

/// Just enough of `manifest.json` to say whether the rest can be read.
#[derive(Deserialize)]
struct FormatProbe {
    format: String,
    format_version: u32,
}

fn read_json<R: Read + Seek, T: DeserializeOwned>(
    archive: &mut ZipArchive<R>,
    name: &'static str,
) -> Result<T, BundleError> {
    let file = archive.by_name(name)?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|source| BundleError::Json { file: name, source })
}

/// An opened bundle: its documents are parsed and checked, and its
/// artwork can be copied out one file at a time.
pub struct BundleReader<R: Read + Seek> {
    archive: ZipArchive<R>,
    artwork: BTreeSet<String>,
}

impl<R: Read + Seek> BundleReader<R> {
    /// Open a bundle. In order: every entry must be one of the six
    /// documents or an artwork file; the manifest must name this
    /// format and version (checked before anything else is parsed, so
    /// a newer bundle says so instead of failing on a field); every
    /// document must parse with no unknown fields; and the whole must
    /// be consistent ([`Bundle::problems`], plus every artwork file
    /// the catalog names present and none unnamed).
    pub fn open(reader: R) -> Result<(Self, Bundle), BundleError> {
        let mut archive = ZipArchive::new(reader)?;
        if archive.has_duplicate_names() {
            return Err(BundleError::NotABundle(
                "the zip holds two files with the same name".into(),
            ));
        }
        let names = archive
            .file_names()
            .map(|name| name.map(|n| n.into_owned()))
            .collect::<Result<Vec<String>, _>>()?;
        let mut documents = BTreeSet::new();
        let mut artwork = BTreeSet::new();
        let mut stray = Vec::new();
        for name in names {
            if let Some(document) = DOCUMENTS.iter().find(|d| **d == name) {
                documents.insert(*document);
            } else if name == ARTWORK_DIR {
                // A folder entry some zip tools add; it holds nothing.
            } else if parse_artwork_name(&name).is_some() {
                artwork.insert(name);
            } else {
                stray.push(name);
            }
        }
        let nested = format!("/{MANIFEST_JSON}");
        if let Some(name) = stray.iter().find(|name| name.ends_with(&nested)) {
            return Err(BundleError::NotABundle(format!(
                "its {MANIFEST_JSON} is inside a folder ({name}); the documents belong at the top of the zip"
            )));
        }
        if let Some(name) = stray.into_iter().next() {
            return Err(BundleError::UnexpectedEntry(name));
        }
        if !documents.contains(MANIFEST_JSON) {
            return Err(BundleError::NotABundle(format!(
                "it has no {MANIFEST_JSON}"
            )));
        }
        let probe: FormatProbe = read_json(&mut archive, MANIFEST_JSON)?;
        if probe.format != FORMAT_NAME {
            return Err(BundleError::NotABundle(format!(
                "its manifest says format '{}'",
                probe.format
            )));
        }
        if probe.format_version != FORMAT_VERSION {
            return Err(BundleError::UnsupportedVersion(probe.format_version));
        }
        if let Some(missing) = DOCUMENTS.iter().find(|d| !documents.contains(**d)) {
            return Err(BundleError::MissingEntry(missing));
        }
        let bundle = Bundle {
            manifest: read_json(&mut archive, MANIFEST_JSON)?,
            users: read_json(&mut archive, USERS_JSON)?,
            profiles: read_json(&mut archive, PROFILES_JSON)?,
            libraries: read_json(&mut archive, LIBRARIES_JSON)?,
            catalog: read_json(&mut archive, CATALOG_JSON)?,
            user_data: read_json(&mut archive, USER_DATA_JSON)?,
        };
        let mut problems = bundle.problems();
        let named = bundle.artwork_files();
        for name in &named {
            if !artwork.contains(*name) {
                problems.push(Problem {
                    file: CATALOG_JSON,
                    message: format!("the artwork file '{name}' is not in the zip"),
                });
            }
        }
        for name in &artwork {
            if !named.contains(name.as_str()) {
                problems.push(Problem {
                    file: ARTWORK_DIR,
                    message: format!("'{name}' is not named by any item in {CATALOG_JSON}"),
                });
            }
        }
        if !problems.is_empty() {
            return Err(BundleError::Invalid(problems));
        }
        Ok((Self { archive, artwork }, bundle))
    }

    /// The artwork files in the bundle, sorted.
    pub fn artwork_names(&self) -> impl Iterator<Item = &str> {
        self.artwork.iter().map(String::as_str)
    }

    /// Copy one artwork file to `out`, checking that its bytes hash to
    /// its name. On a mismatch `out` has already received the bytes —
    /// discard them.
    pub fn copy_artwork<W: Write + ?Sized>(
        &mut self,
        name: &str,
        out: &mut W,
    ) -> Result<u64, BundleError> {
        let Some((sha256, _)) = parse_artwork_name(name).filter(|_| self.artwork.contains(name))
        else {
            return Err(artwork_error(name, "is not in the bundle"));
        };
        let mut file = self.archive.by_name(name)?;
        let mut sink = HashingWriter {
            inner: out,
            hasher: Sha256::new(),
        };
        let copied = io::copy(&mut file, &mut sink)?;
        let digest = format!("{:x}", sink.hasher.finalize());
        if digest != sha256 {
            return Err(artwork_error(name, format!("its bytes hash to {digest}")));
        }
        Ok(copied)
    }
}
