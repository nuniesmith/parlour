//! Parlour server — skeleton binary.
//!
//! Phase 0/1 will grow this into: config loading, database pool +
//! migrations, hardware-backend probing, the native `/api/v1`
//! router, and the embedded SvelteKit web UI.
//! Compatibility routers (Jellyfin / Audiobookshelf) arrive later
//! as views over the same catalog — they own no data.

fn main() {
    println!(
        "parlour {} — skeleton build. See docs/ARCHITECTURE.md for the plan.",
        env!("CARGO_PKG_VERSION")
    );
    // TODO(phase-0): config -> db pool -> migrate -> probe backends -> serve.
}
