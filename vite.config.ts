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

// `tauri dev` cannot reliably kill this server on Ctrl+C. The CLI's cleanup
// (tauri-apps/tauri #10343, #2794, #4262) recursively kills its
// beforeDevCommand child tree, but SIGINT fells the intermediate `npm`/`sh`
// first, the vite process reparents, and the walk finds an already-broken
// chain. The orphan keeps port 8181 and serves a stale module graph to every
// later dev run — observed here as an Aug 28 server still answering four days
// of `tauri dev` sessions. Self-defense: when spawned by the tauri hook chain
// (TAURI_ENV_* is exported to hook commands), watch for *reparenting*. The
// orphaned server's exact signature is its dead parent — `process.ppid` flips
// from the npm/sh chain to init or the user manager — so the poll compares
// the current ppid against the one recorded at startup and shuts down on any
// change. That beats polling the original parent's liveness, which an
// `npx`-style wrapper surviving alongside vite would fool. Manual
// `npm run dev` outside tauri never sets TAURI_ENV_*, so its lifecycle is
// untouched, and in-process config-change restarts keep the same ppid.
function tauriParentWatch(): Plugin {
  return {
    name: "quill-tauri-parent-watch",
    apply: "serve",
    configureServer(server) {
      if (!process.env.TAURI_ENV_PLATFORM) return;
      const parentAtStart = process.ppid;
      const timer = setInterval(() => {
        if (process.ppid === parentAtStart) return;
        clearInterval(timer);
        console.error(
          "[quill] tauri dev chain is gone (reparented) — shutting down dev server",
        );
        void server.close().finally(() => process.exit(0));
      }, 2_000);
      timer.unref();
      server.httpServer?.on("close", () => clearInterval(timer));
    },
  };
}

// Vite's dependency optimizer has no cross-process locking: it writes
// `deps_temp_<hash>/` under `cacheDir` and renames it onto `deps/`. Any second
// process loading this config against the default `node_modules/.vite` — the
// very directory a running `npm run tauri -- dev` owns — re-optimizes into the
// shared cache and races that rename, leaving orphaned temporaries and no
// `deps/` at all. Vite then serves modules whose pre-bundled dependencies 404,
// which shows up as the app rendering with its CSS silently missing — the dev
// server "not reloading". See
// docs/solutions/environment/second-vite-server-strips-dev-css.md.
//
// An allowlist of known offenders (first the test runner, then one-off
// scripts, then hand-started screenshot servers) kept regressing, so the rule
// is inverted: only the repo's own entry may share. That entry is the bare
// `vite` CLI — what `npm run dev` and `tauri dev`'s beforeDevCommand spawn —
// on its configured strict port. Everything else (programmatic
// `createServer()`/`build()` in tests and ad-hoc scripts, or a second CLI run
// with a `--port` override) gets a private per-process cache and cannot touch
// the dev server's.
//
// The private directory lives in the OS temp dir rather than under
// `node_modules`, because these servers set `optimizeDeps.noDiscovery` and
// never commit a `deps/`: every run leaves its temporaries behind, ~39MB a
// time. Somewhere the OS reclaims is the right home for a cache nothing reads
// twice. Per-pid naming isolates even concurrent one-off processes from each
// other. The check reads `process.argv` only — no shell prefix — so it holds
// on Windows, where the `.bin/vite` shim still executes `vite/bin/vite.js`.
function resolveCacheDir(): string | undefined {
  const entry = (process.argv[1] ?? "").replace(/\\/g, "/");
  const isViteCli = entry.endsWith("/vite/bin/vite.js") || entry.endsWith("/.bin/vite");
  const hasPortOverride = process.argv.includes("--port");
  if (isViteCli && !hasPortOverride) return undefined; // shared node_modules/.vite
  return join(tmpdir(), `quill-vite-${process.pid}`);
}

export default defineConfig(({ mode }) => {
  const webBuild = mode === "web";
  const upload = sentryUpload(!webBuild);

  return {
    plugins: [react(), liveDevCsp(), tauriParentWatch(), ...(upload ? [upload] : [])],
    cacheDir: resolveCacheDir(),
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
