import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { sentryVitePlugin } from "@sentry/vite-plugin";
import { resolve } from "path";
import { fileURLToPath } from "url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));

const sentryUpload =
  process.env.SENTRY_AUTH_TOKEN && process.env.NODE_ENV === "production"
    ? sentryVitePlugin({
        org: process.env.SENTRY_ORG ?? "stable-tech",
        project: process.env.SENTRY_PROJECT ?? "quill",
        authToken: process.env.SENTRY_AUTH_TOKEN,
        telemetry: false,
        release: {
          name: process.env.SENTRY_RELEASE || undefined,
        },
      })
    : null;

export default defineConfig({
  plugins: [react(), ...(sentryUpload ? [sentryUpload] : [])],
  clearScreen: false,
  server: {
    port: 8181,
    strictPort: true,
    hmr: {
      protocol: "ws",
      host: "localhost",
      port: 8181,
    },
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: true,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
      },
    },
  },
});
