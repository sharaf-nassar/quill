// Deny-by-default crash reporter (frontend half).
//
// Mirrors src-tauri/src/crash_reporting.rs. The SDK is NOT initialized at
// module load — the bootstrap in main.tsx waits for the user's preference
// from get_runtime_settings before opting in. Toggling the setting at runtime
// emits "runtime-settings-updated" which the listener below honors so the
// transport closes immediately when the user opts out.

import * as Sentry from "@sentry/react";
import type { ErrorEvent, Breadcrumb } from "@sentry/react";
import { listen } from "@tauri-apps/api/event";
import type { RuntimeSettings } from "../types";

const DSN =
  "https://8b9ef3ae161eb57fe9df88bb446fe0a1@o1373069.ingest.us.sentry.io/4511465093267456";

const ALLOWED_TAG_KEYS = new Set(["release", "environment", "runtime"]);
const ALLOWED_CONTEXT_KEYS = new Set([
  "os",
  "device",
  "runtime",
  "app",
  "browser",
]);

let initialized = false;

export function setCrashReportingEnabled(enabled: boolean): void {
  if (enabled === initialized) return;
  if (enabled) {
    Sentry.init({
      dsn: DSN,
      environment: import.meta.env.MODE,
      release: import.meta.env.VITE_APP_VERSION,
      sendDefaultPii: false,
      defaultIntegrations: false,
      integrations: [
        Sentry.globalHandlersIntegration(),
        Sentry.functionToStringIntegration(),
        Sentry.inboundFiltersIntegration(),
        Sentry.dedupeIntegration(),
      ],
      maxBreadcrumbs: 0,
      attachStacktrace: true,
      beforeSend,
      beforeBreadcrumb,
      initialScope: {
        tags: { runtime: "frontend" },
      },
    });
    initialized = true;
  } else {
    void Sentry.close();
    initialized = false;
  }
}

// Deny-by-default scrubber. Keeps only stack-frame structure + allowlisted
// tags/contexts; strips dynamic content (messages, breadcrumbs, request data,
// user, extra) and reduces filenames to basenames so a developer's local
// $HOME never appears in a path.
function beforeSend(event: ErrorEvent): ErrorEvent | null {
  if (event.message) event.message = "[scrubbed]";
  event.logentry = undefined;
  event.fingerprint = undefined;

  if (event.exception?.values) {
    event.exception.values = event.exception.values.map((v) => ({
      type: v.type,
      value: "[scrubbed]",
      mechanism: v.mechanism,
      stacktrace: v.stacktrace
        ? {
            ...v.stacktrace,
            frames: v.stacktrace.frames?.map((f) => ({
              function: f.function,
              module: f.module,
              filename: basename(f.filename),
              abs_path: basename(f.abs_path),
              lineno: f.lineno,
              colno: f.colno,
              in_app: f.in_app,
            })),
          }
        : undefined,
    }));
  }

  event.breadcrumbs = [];
  delete event.user;
  delete event.request;
  delete event.extra;
  event.tags = pickAllowed(event.tags, ALLOWED_TAG_KEYS);
  event.contexts = pickAllowed(event.contexts, ALLOWED_CONTEXT_KEYS);

  return event;
}

function beforeBreadcrumb(_b: Breadcrumb): Breadcrumb | null {
  return null;
}

function basename(path?: string): string | undefined {
  if (!path) return undefined;
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function pickAllowed<T extends Record<string, unknown> | undefined>(
  obj: T,
  allowed: Set<string>,
): T {
  if (!obj) return obj;
  const result: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    if (allowed.has(k)) result[k] = v;
  }
  return result as T;
}

void listen<RuntimeSettings>("runtime-settings-updated", (event) => {
  setCrashReportingEnabled(event.payload.crashReportingEnabled);
});
