// Deny-by-default crash reporter.
//
// Every Quill session is sensitive: file paths, prompt text, and session
// transcripts can all show up in panic messages or exception values. The
// scrubber below strips dynamic content from every outgoing event and keeps
// only the skeletal stack frame structure (function/module/line). The toggle
// in Settings → General is the user-facing opt-out; when disabled, the
// `ClientInitGuard` is dropped which flushes pending events and closes the
// transport.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use sentry::ClientInitGuard;
use sentry::protocol::Event;
use std::sync::Mutex;

const DSN: &str =
    "https://8b9ef3ae161eb57fe9df88bb446fe0a1@o1373069.ingest.us.sentry.io/4511465093267456";
const RELEASE: &str = concat!("v", env!("CARGO_PKG_VERSION"));

static GUARD: OnceLock<Mutex<Option<ClientInitGuard>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<ClientInitGuard>> {
    GUARD.get_or_init(|| Mutex::new(None))
}

pub fn set_enabled(enabled: bool) {
    if enabled {
        enable();
    } else {
        disable();
    }
}

fn enable() {
    let mut g = slot().lock().unwrap();
    if g.is_some() {
        return;
    }
    let environment = if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    };
    let guard = sentry::init((
        DSN,
        sentry::ClientOptions {
            release: Some(RELEASE.into()),
            environment: Some(environment.into()),
            attach_stacktrace: true,
            send_default_pii: false,
            auto_session_tracking: false,
            max_breadcrumbs: 0,
            before_send: Some(Arc::new(scrub_event)),
            before_breadcrumb: Some(Arc::new(|_| None)),
            ..Default::default()
        },
    ));
    sentry::configure_scope(|scope| {
        scope.set_tag("runtime", "rust");
    });
    *g = Some(guard);
}

fn disable() {
    let _ = slot().lock().unwrap().take();
}

// before_send hook — runs for every outgoing event. Deny-by-default: strip
// every field that can carry user data, keep only stack-frame structure and
// allowlisted tags.
fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    event.message = None;
    event.logentry = None;
    event.fingerprint = std::borrow::Cow::Borrowed(&[]);
    event.server_name = None;

    for exception in event.exception.values.iter_mut() {
        exception.value = None;
        if let Some(stacktrace) = exception.stacktrace.as_mut() {
            for frame in stacktrace.frames.iter_mut() {
                frame.vars.clear();
                frame.pre_context.clear();
                frame.post_context.clear();
                frame.context_line = None;
                if let Some(path) = frame.filename.as_mut() {
                    *path = basename(path);
                }
                if let Some(abs) = frame.abs_path.as_mut() {
                    *abs = basename(abs);
                }
            }
        }
    }

    event.breadcrumbs.values.clear();
    event.user = None;
    event.request = None;
    event.extra.clear();
    event
        .tags
        .retain(|key, _| matches!(key.as_str(), "release" | "environment" | "runtime"));
    event
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "rust" | "app"));

    Some(event)
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // @lat: [[crash-reporting-tests#Crash Reporting Test Specs#Shared tagged release identifier]]
    #[test]
    fn release_matches_tagged_cargo_version() {
        assert_eq!(RELEASE, format!("v{}", env!("CARGO_PKG_VERSION")));
    }

    // @lat: [[crash-reporting-tests#Crash Reporting Test Specs#Rust deny-by-default payload boundary]]
    #[test]
    fn scrub_event_keeps_only_approved_diagnostics() {
        let event: Event<'static> = serde_json::from_value(json!({
            "server_name": "alice-workstation",
            "release": "quill@1.2.3",
            "environment": "production",
            "message": "SESSION_SECRET",
            "logentry": { "message": "SESSION_SECRET", "params": ["SESSION_SECRET"] },
            "exception": {
                "values": [{
                    "type": "panic",
                    "value": "SESSION_SECRET",
                    "stacktrace": {
                        "frames": [{
                            "function": "quill::run",
                            "filename": "/home/alice/private/main.rs",
                            "abs_path": "/home/alice/private/main.rs",
                            "vars": { "prompt": "SESSION_SECRET" },
                            "pre_context": ["SESSION_SECRET"],
                            "context_line": "SESSION_SECRET",
                            "post_context": ["SESSION_SECRET"]
                        }]
                    }
                }]
            },
            "breadcrumbs": { "values": [{ "message": "SESSION_SECRET" }] },
            "request": { "url": "https://example.test/SESSION_SECRET" },
            "user": { "email": "alice@example.test" },
            "extra": { "prompt": "SESSION_SECRET" },
            "tags": {
                "release": "quill@1.2.3",
                "environment": "production",
                "runtime": "rust",
                "project_path": "/home/alice/private"
            },
            "contexts": {
                "os": { "type": "os", "name": "Linux" },
                "device": { "type": "device", "arch": "x86_64" },
                "rust": { "type": "runtime", "name": "rustc" },
                "app": { "type": "app", "app_name": "Quill" },
                "custom": { "type": "unknown", "prompt": "SESSION_SECRET" }
            }
        }))
        .expect("deserialize representative Sentry event");

        let scrubbed = scrub_event(event).expect("scrubber keeps event");
        let frame = &scrubbed.exception.values[0]
            .stacktrace
            .as_ref()
            .expect("stacktrace retained")
            .frames[0];

        assert_eq!(scrubbed.release.as_deref(), Some("quill@1.2.3"));
        assert_eq!(scrubbed.environment.as_deref(), Some("production"));
        assert_eq!(scrubbed.tags.len(), 3);
        assert_eq!(
            scrubbed.tags.get("runtime").map(String::as_str),
            Some("rust")
        );
        assert_eq!(scrubbed.contexts.len(), 4);
        assert!(scrubbed.contexts.contains_key("rust"));
        assert_eq!(frame.filename.as_deref(), Some("main.rs"));
        assert_eq!(frame.abs_path.as_deref(), Some("main.rs"));

        let payload = serde_json::to_string(&scrubbed).expect("serialize scrubbed event");
        for forbidden in [
            "alice-workstation",
            "SESSION_SECRET",
            "/home/alice",
            "alice@example.test",
            "project_path",
            "custom",
        ] {
            assert!(!payload.contains(forbidden), "payload leaked {forbidden}");
        }
    }
}
