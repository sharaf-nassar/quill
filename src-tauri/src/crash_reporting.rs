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

use parking_lot::Mutex;
use sentry::ClientInitGuard;
use sentry::protocol::Event;

const DSN: &str =
    "https://8b9ef3ae161eb57fe9df88bb446fe0a1@o1373069.ingest.us.sentry.io/4511465093267456";

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
    let mut g = slot().lock();
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
            release: sentry::release_name!(),
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
    let _ = slot().lock().take();
}

// before_send hook — runs for every outgoing event. Deny-by-default: strip
// every field that can carry user data, keep only stack-frame structure and
// allowlisted tags.
fn scrub_event(mut event: Event<'static>) -> Option<Event<'static>> {
    event.message = None;
    event.logentry = None;
    event.fingerprint = std::borrow::Cow::Borrowed(&[]);

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
        .contexts
        .retain(|key, _| matches!(key.as_str(), "os" | "device" | "runtime" | "app"));

    Some(event)
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned()
}
