//! The browser bundle the authenticated request class is allowed to download.
//!
//! Isolation is structural rather than filtered: the desktop bundle builds from
//! `index.html` into `dist/` and the monitor bundle builds from `web.html` into
//! `dist-web/`, so the Manage and Release Notes chunks `src/main.tsx` imports
//! are never emitted into the folder this module embeds. Nothing here inspects
//! or excludes chunk names, because no such chunk exists to reach.

use axum::{
    Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed::RustEmbed;

/// Release builds embed `dist-web`; debug builds read it from disk on every
/// request, which is what lets `npm run tauri -- dev` serve a bundle Vite
/// rebuilt after the Rust binary was compiled.
///
/// The folder must exist when the crate compiles. `npm run build:web` is wired
/// into both Tauri lifecycle commands and into CI ahead of the Rust build, so a
/// missing folder is a workflow that skipped it — a compile error naming the
/// path is a better answer than a binary that silently serves nothing.
#[derive(RustEmbed)]
#[folder = "../dist-web"]
struct WebBundle;

/// The bundle's entry document, which Vite names after its `web.html` input
/// rather than `index.html`.
const DOCUMENT: &str = "web.html";

/// Serve the monitor's asset graph.
///
/// The monitor surface is one document with no client-side router, so the SPA
/// fallback set is exactly `/` — mounted separately by
/// [[src-tauri/src/web_server/router.rs#routes]] because an unpaired browser is
/// redirected there rather than refused. Every other path resolves against real
/// files under `assets/` and otherwise falls through to the caller's `404`: a
/// browser cannot reach the entry document by asking for an arbitrary path, and
/// no build artifact outside `assets/` — the Vite manifest included — is
/// addressable at all.
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/assets/{*path}", get(asset))
}

pub(super) async fn document() -> Response {
    serve(DOCUMENT)
}

/// `path` needs no traversal check of its own: a release lookup is an exact key
/// match against the embedded set, and the debug lookup canonicalizes the
/// candidate and refuses anything outside the folder.
async fn asset(Path(path): Path<String>) -> Response {
    serve(&format!("assets/{path}"))
}

fn serve(path: &str) -> Response {
    let Some(file) = WebBundle::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
        file.data,
    )
        .into_response()
}
