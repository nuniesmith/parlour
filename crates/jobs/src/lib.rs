//! Parlour background jobs (placeholder).
//!
//! A Postgres-backed job table claimed with
//! `FOR UPDATE SKIP LOCKED`. No Redis until Postgres demonstrably
//! cannot keep up (it will, at household scale).
//!
//! Jobs are how scanning follow-ups, subtitle extraction and
//! downloads, thumbnail/artwork generation, and request
//! reconciliation stay out of request paths.

/// A kind of background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    ScanLibrary,
    ExtractSubtitles,
    DownloadSubtitles,
    GenerateArtwork,
    ReconcileRequests,
}
