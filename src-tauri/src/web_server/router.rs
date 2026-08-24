//! `/pair`, `POST /api/web/pair`, and `POST /api/web/invoke` handlers.
//!
//! Mounting is itself the enforcement of the three request classes in
//! `specs/029-web-ui-server.md#api--interface-changes`: the pairing page and
//! the pairing POST are the only public routes, and every other path —
//! including asset paths and paths this router does not mount at all — reaches
//! a handler only behind
//! [[src-tauri/src/web_server/gates.rs#require_session]]. The pairing page is
//! rendered here rather than served from the bundle so an unpaired browser can
//! bootstrap without ever receiving an application chunk.

use std::sync::{Arc, LazyLock};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::{
    integrations::IntegrationProvider,
    live_tracker::LiveTracker,
    web_allowlist::is_permitted_command,
    web_pairing,
    web_server::{
        InvokeRequest, InvokeResponse, PairRequest, WebServerState, assets,
        gates::{enforce_pairing_budget, refused, require_session},
        session_cookie,
    },
};

/// Mount the three request classes. The authenticated router owns the fallback
/// so an unrouted path is refused by the session gate rather than answered.
pub fn routes(state: Arc<WebServerState>) -> Router {
    let public = Router::new().route("/pair", get(pair_page)).route(
        "/api/web/pair",
        post(pair).layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            enforce_pairing_budget,
        )),
    );
    let authenticated = Router::new()
        .route("/api/web/invoke", post(invoke))
        .merge(assets::routes())
        .fallback(unserved)
        .layer(middleware::from_fn(require_session))
        .with_state(state);
    public.merge(authenticated)
}

async fn pair_page() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (header::CONTENT_SECURITY_POLICY, PAIR_PAGE_CSP.clone()),
        ],
        PAIR_PAGE.as_str(),
    )
        .into_response()
}

/// Exchange the pairing code for a session cookie.
///
/// A body that is not the contracted request is refused exactly like a wrong
/// code: both mean this caller did not present the credential, and answering
/// them differently would tell an attacker which half it got wrong.
async fn pair(body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<PairRequest>(&body) else {
        return refused();
    };
    match web_pairing::verify_pairing_code(&request.code) {
        Ok(true) => {}
        Ok(false) => return refused(),
        Err(error) => {
            log::error!("Web pairing could not read the pairing credential: {error}");
            return refused();
        }
    }
    match web_pairing::issue_session() {
        Ok(token) => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, session_cookie(&token))],
        )
            .into_response(),
        Err(error) => {
            log::error!("Web pairing could not issue a session: {error}");
            refused()
        }
    }
}

/// Dispatch one browser invoke.
///
/// The allowlist decides admission before the arguments are even decoded, so a
/// denied command cannot reach a shape that would tell the caller whether it
/// exists.
async fn invoke(State(state): State<Arc<WebServerState>>, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<InvokeRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !is_permitted_command(&request.cmd) {
        return envelope(InvokeResponse::command_denied());
    }
    let Ok(command) = serde_json::from_slice::<WebCommand>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    envelope(match dispatch(&state, command).await {
        Ok(value) => InvokeResponse::success(value),
        Err(message) => InvokeResponse::command_error(message),
    })
}

/// A path outside the invoke route and the bundle's own asset graph is a plain
/// miss rather than a refusal, because the caller already proved it is paired.
async fn unserved() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn envelope(response: InvokeResponse) -> Response {
    let status = StatusCode::from_u16(response.status()).expect("contracted invoke status");
    (status, Json(response)).into_response()
}

/// The decoded argument shape of each permitted command.
///
/// Variant names are the complete invoke command strings and the fields are the
/// desktop command's own parameters under Tauri's camelCase argument
/// convention, so the shared widget call sites reach the same signatures they
/// reach in the app. Admission stays with
/// [[src-tauri/src/web_allowlist.rs#is_permitted_command]]; this type only
/// decodes what that boundary already let through.
#[derive(Deserialize)]
#[serde(
    tag = "cmd",
    content = "args",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
// The shared `Get` prefix is the wire contract, not a naming habit: each
// variant name lowers to the exact invoke command string.
#[allow(clippy::enum_variant_names)]
enum WebCommand {
    GetActivitySeries {
        range: String,
        buckets: Option<u32>,
    },
    GetCachedUsageData {},
    GetCodeStats {
        range: String,
    },
    GetCodeStatsHistory {
        range: String,
    },
    GetContextSavingsAnalytics {
        range: String,
        limit: Option<i64>,
    },
    GetCpaConnectionStatus {},
    GetHookBreakdown {
        range: String,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i32>,
    },
    GetHostBreakdown {
        range: String,
    },
    GetLlmRuntimeStats {
        range: String,
        scope: Option<String>,
    },
    GetModelUsageOverview {
        range: String,
        provider: Option<String>,
    },
    GetProjectBreakdown {
        range: String,
    },
    GetProviderStatuses {},
    GetRetentionPolicy {},
    GetSessionBreakdown {
        range: String,
        hostname: Option<String>,
        provider: Option<IntegrationProvider>,
        limit: Option<i32>,
    },
    GetSkillBreakdown {
        range: String,
        provider: Option<IntegrationProvider>,
        all_time: bool,
        limit: Option<i32>,
    },
    GetTokenHistory {
        range: String,
        provider: Option<IntegrationProvider>,
        hostname: Option<String>,
        session_id: Option<String>,
        cwd: Option<String>,
    },
}

/// Run one permitted command against the same desktop implementation the app
/// window calls, so the browser cannot see a second, divergent read path.
async fn dispatch(state: &WebServerState, command: WebCommand) -> Result<Value, String> {
    match command {
        WebCommand::GetActivitySeries { range, buckets } => {
            encode(crate::get_activity_series(range, buckets).await)
        }
        WebCommand::GetCachedUsageData {} => encode(crate::get_cached_usage_data().await),
        WebCommand::GetCodeStats { range } => encode(crate::get_code_stats(range).await),
        WebCommand::GetCodeStatsHistory { range } => {
            encode(crate::get_code_stats_history(range).await)
        }
        WebCommand::GetContextSavingsAnalytics { range, limit } => {
            encode(crate::get_context_savings_analytics(range, limit).await)
        }
        WebCommand::GetCpaConnectionStatus {} => {
            encode(crate::get_cpa_connection_status().map_err(|error| error.message))
        }
        WebCommand::GetHookBreakdown {
            range,
            provider,
            all_time,
            limit,
        } => encode(crate::get_hook_breakdown(range, provider, all_time, limit).await),
        WebCommand::GetHostBreakdown { range } => encode(crate::get_host_breakdown(range).await),
        WebCommand::GetLlmRuntimeStats { range, scope } => {
            encode(crate::get_llm_runtime_stats(range, scope).await)
        }
        WebCommand::GetModelUsageOverview { range, provider } => encode(
            crate::get_model_usage_overview(range, provider)
                .await
                .map_err(|error| error.to_string()),
        ),
        WebCommand::GetProjectBreakdown { range } => {
            encode(crate::get_project_breakdown(range).await)
        }
        WebCommand::GetProviderStatuses {} => encode(crate::get_provider_statuses().await),
        WebCommand::GetRetentionPolicy {} => encode(crate::get_retention_policy().await),
        WebCommand::GetSessionBreakdown {
            range,
            hostname,
            provider,
            limit,
        } => {
            // The live overlay is Tauri-managed state, so the browser's read
            // resolves the same tracker the desktop window reads.
            let tracker = state
                .app
                .as_ref()
                .and_then(|app| app.try_state::<Arc<LiveTracker>>());
            let Some(tracker) = tracker else {
                return Err("Session activity is unavailable.".to_string());
            };
            encode(crate::get_session_breakdown(range, hostname, provider, limit, tracker).await)
        }
        WebCommand::GetSkillBreakdown {
            range,
            provider,
            all_time,
            limit,
        } => encode(crate::get_skill_breakdown(range, provider, all_time, limit).await),
        WebCommand::GetTokenHistory {
            range,
            provider,
            hostname,
            session_id,
            cwd,
        } => encode(crate::get_token_history(range, provider, hostname, session_id, cwd).await),
    }
}

fn encode<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
    serde_json::to_value(result?).map_err(|error| {
        log::error!("A web invoke result could not be encoded: {error}");
        "Quill could not encode this result.".to_string()
    })
}

const PAIR_STYLE: &str = "\
:root{color-scheme:dark}\
*{box-sizing:border-box}\
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:24px;background:#0d1117;color:#c9d1d9;font:14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif}\
main{width:100%;max-width:380px;padding:28px 24px;border:1px solid #21262d;background:#161b22}\
.mark{margin:0 0 20px;font-size:11px;letter-spacing:.18em;color:#6e7681}\
h1{margin:0 0 8px;font-size:17px;font-weight:600;color:#e6edf3}\
.note{margin:0 0 22px;color:#8b949e}\
label{display:block;margin-bottom:6px;font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:#8b949e}\
input{width:100%;padding:10px 12px;border:1px solid #21262d;border-radius:2px;background:#0f1319;color:#e6edf3;font:14px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace}\
input:focus{outline:none;border-color:#60a5fa}\
button{width:100%;margin-top:16px;padding:10px 12px;border:1px solid #30363d;border-radius:2px;background:#1e1e24;color:#e6edf3;font:inherit;cursor:pointer}\
button:hover:enabled{background:#21262d}\
button:disabled{opacity:.55;cursor:default}\
.failure{margin:16px 0 0;color:#f87171}";

const PAIR_SCRIPT: &str = r#"const form = document.getElementById("pair");
const code = document.getElementById("code");
const failure = document.getElementById("failure");
const submit = document.getElementById("submit");
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  failure.hidden = true;
  submit.disabled = true;
  try {
    const response = await fetch("/api/web/pair", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: code.value.trim() }),
    });
    if (response.status === 204) {
      location.replace("/");
      return;
    }
  } catch (error) {
    console.error(error);
  }
  failure.hidden = false;
  submit.disabled = false;
  code.select();
});"#;

/// The bootstrap page an unpaired browser is allowed to see: no Quill data, no
/// reference to any bundle chunk, and nothing beyond the pairing exchange.
///
/// The form names its own method and action so a submit that outruns the script
/// posts the code in a body; the default would put the credential in a URL.
static PAIR_PAGE: LazyLock<String> = LazyLock::new(|| {
    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>Pair with Quill</title>\n\
<style>{PAIR_STYLE}</style>\n\
</head>\n\
<body>\n\
<main>\n\
<p class=\"mark\">QUILL</p>\n\
<h1>Pair this browser</h1>\n\
<p class=\"note\">Open Quill on the desktop, then enter the pairing code from Settings \u{2192} Web.</p>\n\
<form id=\"pair\" method=\"post\" action=\"/api/web/pair\">\n\
<label for=\"code\">Pairing code</label>\n\
<input id=\"code\" name=\"code\" type=\"text\" autocomplete=\"off\" autocapitalize=\"off\" autocorrect=\"off\" spellcheck=\"false\" required>\n\
<button id=\"submit\" type=\"submit\">Pair browser</button>\n\
</form>\n\
<p id=\"failure\" class=\"failure\" role=\"alert\" hidden>That pairing code was not accepted. Check the code in Settings \u{2192} Web and try again.</p>\n\
</main>\n\
<script>{PAIR_SCRIPT}</script>\n\
</body>\n\
</html>\n"
    )
});

/// The page's own policy. Its one inline script is pinned by hash so the page
/// keeps the bundle's `script-src` guarantee without loading a chunk.
static PAIR_PAGE_CSP: LazyLock<HeaderValue> = LazyLock::new(|| {
    let digest = STANDARD.encode(Sha256::digest(PAIR_SCRIPT));
    HeaderValue::from_str(&format!(
        "default-src 'none'; style-src 'unsafe-inline'; script-src 'sha256-{digest}'; connect-src 'self'"
    ))
    .expect("pairing page policy is a valid header value")
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_server::gates::{BoundedListener, WebPeer};
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;

    /// Serve the real router over loopback, which is the only way to present a
    /// peer address the gates will accept: identity comes from the accepted
    /// socket, never from a header.
    async fn serve() -> String {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind router test listener");
        let address = listener.local_addr().expect("router test address");
        let app = crate::web_server::router(Arc::new(WebServerState::default()));
        tokio::spawn(async move {
            axum::serve(
                BoundedListener::new(listener),
                app.into_make_service_with_connect_info::<WebPeer>(),
            )
            .await
            .expect("serve router test");
        });
        format!("http://{address}")
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Unpaired access reaches only the pairing bootstrap]]
    #[tokio::test]
    async fn an_unpaired_peer_reaches_only_the_pairing_page() {
        let base = serve().await;
        let client = reqwest::Client::new();

        let page = client
            .get(format!("{base}/pair"))
            .send()
            .await
            .expect("pairing page");
        assert_eq!(page.status(), StatusCode::OK);
        let policy = page
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .expect("pairing page policy")
            .to_string();
        assert!(policy.contains("script-src 'sha256-"), "{policy}");
        let body = page.text().await.expect("pairing page body");
        assert!(body.contains("Pair this browser"));
        // The page bootstraps pairing; it must reference no bundle chunk and
        // carry no Quill data of its own.
        assert!(!body.contains("src="), "{body}");
        assert!(!body.contains("/assets/"), "{body}");

        for (method, path) in [
            (reqwest::Method::GET, "/"),
            (reqwest::Method::GET, "/assets/app.js"),
            (reqwest::Method::POST, "/api/web/invoke"),
            (reqwest::Method::GET, "/does-not-exist"),
            // A pairing body that is not the contracted request is refused
            // exactly like a wrong code.
            (reqwest::Method::POST, "/api/web/pair"),
        ] {
            let response = client
                .request(method.clone(), format!("{base}{path}"))
                .body("not the contracted request")
                .send()
                .await
                .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
            assert!(
                response.bytes().await.expect("refusal body").is_empty(),
                "{method} {path} must refuse before reading any Quill data"
            );
        }
    }
}
