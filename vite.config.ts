import { tmpdir } from "node:os";
import { join } from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { sentryVitePlugin } from "@sentry/vite-plugin";

function sentryUpload(enabled: boolean) {
  return enabled && process.env.SENTRY_AUTH_TOKEN && process.env.NODE_ENV === "production"
    ? sentryVitePlugin({
        org: process.env.SENTRY_ORG ?? "stable-tech",
        project: process.env.SENTRY_PROJECT ?? "quill",
        authToken: process.env.SENTRY_AUTH_TOKEN,
        telemetry: false,
        errorHandler(error) {
          throw error;
        },
        release: {
          name: process.env.SENTRY_RELEASE || undefined,
          create: false,
          finalize: false,
          setCommits: false,
        },
        sourcemaps: {
          filesToDeleteAfterUpload: "./dist/**/*.map",
        },
      })
    : null;
}

// Dev-only CSP relaxation. The production index.html ships a strict Tauri CSP
// (script-src 'self'; connect-src limited to IPC and Sentry). It blocks Vite HMR,
// React Fast Refresh, and the Impeccable live client (http://localhost:8400) in
// a plain browser. We swap in a dev-friendly policy in `vite serve` only. Because
// the plugin is `apply: "serve"`, `vite build` (used by `tauri build`) never runs
// it, so the shipped production CSP is left exactly as-is.
function liveDevCsp(): Plugin {
  const DEV_CSP = [
    "default-src 'self'",
    "script-src 'self' 'unsafe-inline' 'unsafe-eval' http://localhost:8400",
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    "connect-src 'self' ws://localhost:8181 ws://localhost:8400 http://localhost:8400 ipc: http://ipc.localhost https://ipc.localhost https://o1373069.ingest.us.sentry.io",
  ].join("; ");
  return {
    name: "quill-live-dev-csp",
    apply: "serve",
    transformIndexHtml(html) {
      return html.replace(
        /(<meta http-equiv="Content-Security-Policy" content=")[^"]*(")/,
        `$1${DEV_CSP};$2`,
      );
    },
  };
}

// Vite's dependency optimizer has no cross-process locking: it writes
// `deps_temp_<hash>/` under `cacheDir` and renames it onto `deps/`. Eleven test
// files call `createServer()` and one calls `build()`, each loading this config
// and each defaulting to `node_modules/.vite` — the very directory a running
// `npm run tauri -- dev` owns. Their config hash differs from the dev server's,
// so they re-optimize into the shared cache and race that rename, leaving
// orphaned temporaries and no `deps/` at all. Vite then serves modules whose
// pre-bundled dependencies 404, which shows up as the app rendering with its
// CSS silently missing — the dev server "not reloading".
//
// Node sets `NODE_TEST_CONTEXT` in every `node --test` child; the filename check
// is a second, repo-owned signal so this keeps holding if that variable ever
// changes. Either one moves the test servers onto their own cache directory,
// with no shell prefix that would break on Windows.
//
// That directory lives in the OS temp dir rather than under `node_modules`,
// because these servers set `optimizeDeps.noDiscovery` and never commit a
// `deps/`: every run leaves its temporaries behind, ~39MB a time. Somewhere the
// OS reclaims is the right home for a cache nothing reads twice.
function testCacheDir(): string | undefined {
  const isTestRunner =
    Boolean(process.env.NODE_TEST_CONTEXT) ||
    Boolean(process.argv[1]?.endsWith(".test.mjs"));
  return isTestRunner ? join(tmpdir(), "quill-vite-test-cache") : undefined;
}

export default defineConfig(({ mode }) => {
  const webBuild = mode === "web";
  const upload = sentryUpload(!webBuild);

  return {
    plugins: [react(), liveDevCsp(), ...(upload ? [upload] : [])],
    cacheDir: testCacheDir(),
    clearScreen: false,
    server: {
      host: "0.0.0.0",
      allowedHosts: true,
      port: 8181,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**", "**/.worktrees/**"],
      },
    },
    build: {
      target: "esnext",
      sourcemap: Boolean(upload),
      chunkSizeWarningLimit: 550,
      ...(webBuild
        ? {
            outDir: "dist-web",
            rollupOptions: { input: "web.html" },
            // The manifest is the oracle for what the web listener embeds:
            // it names the entry's whole transitive chunk graph, so a test can
            // prove the served asset set is that graph and nothing else. It
            // lands in `.vite/`, which the listener does not route.
            manifest: true,
          }
        : {}),
    },
  };
});
