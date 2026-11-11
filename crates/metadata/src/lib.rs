//! Parlour metadata providers (placeholder).
//!
//! Providers sit behind a trait so a new library type means adding
//! a provider, not touching the catalog. Planned first: TMDB and
//! TheTVDB. Later, with their library types: MusicBrainz
//! (music), Audnexus (audiobooks), OpenLibrary (ebooks).

/// A metadata source the catalog can match against.
pub trait MetadataProvider {
    /// Stable short name, e.g. "tmdb".
    fn name(&self) -> &'static str;
}
