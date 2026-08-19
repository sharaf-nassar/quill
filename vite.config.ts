import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { sentryVitePlugin } from "@sentry/vite-plugin";

const sentryUpload =
  process.env.SENTRY_AUTH_TOKEN && process.env.NODE_ENV === "production"
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

export default defineConfig({
  plugins: [react(), liveDevCsp(), ...(sentryUpload ? [sentryUpload] : [])],
  clearScreen: false,
  server: {
    port: 8181,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/.worktrees/**"],
    },
  },
  build: {
    target: "esnext",
    sourcemap: Boolean(sentryUpload),
    chunkSizeWarningLimit: 550,
  },
});
