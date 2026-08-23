//! Shared types and router assembly for the app-served web UI.
//!
//! Request handlers and listener lifecycle intentionally land in later web UI
//! work items. Keeping the router state here gives those modules one ownership
//! boundary without starting a listener during this scaffold.

use std::sync::Arc;

use axum::Router;
use serde::Serialize;

/// State shared by all web UI routes.
///
/// Fields are added by the controller, credential, and request-gate work items.
#[derive(Clone, Default)]
pub struct WebServerState;

/// Build the web UI router from its shared state.
///
/// No routes are mounted until their request-gate and transport contracts land.
pub fn router(state: Arc<WebServerState>) -> Router {
    Router::new().with_state(state)
}

/// Display-safe error returned while the web UI scaffold has no implementation.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebUiError {
    pub code: WebUiErrorCode,
    pub message: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiErrorCode {
    NotImplemented,
}

impl WebUiError {
    pub fn not_implemented() -> Self {
        Self {
            code: WebUiErrorCode::NotImplemented,
            message: "Web UI server is not implemented yet.",
        }
    }
}
