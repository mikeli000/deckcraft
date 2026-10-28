//! Stateless HTTP proxy for Seismic LiveDocs "Upload Live": holds `SEISMIC_*` credentials
//! (`deckcraft_livedoc::upload`) so `deckcraft-web`'s browser build never sees them. The browser
//! sends an already-exported livedoc `.pptx` (`deckcraft_livedoc::export_pptx_with_livedoc`,
//! which the browser can already produce on its own — no credentials needed for that part); this
//! server re-exports it with the real tenant's upload skeleton
//! (`deckcraft_livedoc::export_pptx_for_live_upload`) and calls Seismic on its behalf.
//!
//! Unlike monear's `doc-server`, this holds no document state of its own: DeckCraft's document
//! always lives in the client (the desktop app's `Session`, or the browser's wasm heap), never on
//! this server — there is nothing here to persist, back up or leak beyond the one request in
//! flight. One route, `POST /livedoc/upload`.
//!
//! Run locally alongside `trunk serve` (`apps/deckcraft-web/Trunk.toml`'s `[[proxy]]` forwards
//! `/livedoc/upload` here, so the browser's request stays same-origin — no CORS round trip
//! needed for local dev, though this still sets a permissive CORS header as a fallback for
//! hitting this server directly).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use axum::Router;
use axum::extract::{DefaultBodyLimit, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;

/// 200 MB — matches monear's own `doc-server`; a presentation with embedded media can be large.
const MAX_BODY_BYTES: usize = 200 * 1024 * 1024;

#[derive(Deserialize, Default)]
struct UploadQuery {
    #[serde(default)]
    name: String,
}

/// `POST /livedoc/upload?name=...`: `body` is a `.pptx` already exported with
/// `export_pptx_with_livedoc` (any `AD_HOC` variables it has are already in there); `name` is the
/// content's display name in the Library, defaulting to `"deckcraft-livedoc"` when blank.
async fn upload_live(Query(q): Query<UploadQuery>, body: axum::body::Bytes) -> Response {
    let creds = match deckcraft_livedoc::upload::SeismicCredentials::from_env() {
        Ok(c) => c,
        Err(e) => return (StatusCode::NOT_IMPLEMENTED, format!("Upload Live isn't configured on this server: {e}")).into_response(),
    };
    let presentation = match deckcraft_pptx::import(&body) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("not a .pptx this server can read: {e}")).into_response(),
    };
    let bytes = match deckcraft_livedoc::export_pptx_for_live_upload(&presentation, &creds.tenant) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to export .pptx: {e}")).into_response(),
    };
    let name = if q.name.trim().is_empty() { "deckcraft-livedoc".to_string() } else { q.name };
    let filename = format!("{name}.pptx");
    let upload = tokio::task::spawn_blocking(move || deckcraft_livedoc::upload::upload_live_pptx(&creds, bytes, &filename, &name)).await;
    match upload {
        Ok(Ok(result)) => axum::Json(result).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_GATEWAY, format!("Upload Live failed: {e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("upload task didn't finish: {e}")).into_response(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A missing `.env` is not an error — every route here still works without one, just
    // `upload_live` reports 501 until `SEISMIC_*` is set some other way.
    let _ = dotenvy::dotenv();
    let port: u16 = std::env::var("DECKCRAFT_LIVEDOC_SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(4100);
    let app = Router::new()
        .route("/livedoc/upload", post(upload_live))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        // Dev-only: tighten this before deploying anywhere real. Local dev doesn't need it at
        // all (Trunk.toml's proxy keeps the browser's request same-origin); this only matters
        // for hitting this server directly (curl, Postman) or a setup without that proxy.
        .layer(tower_http::cors::CorsLayer::permissive());
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("deckcraft-server: Upload Live proxy listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
