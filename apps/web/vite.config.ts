import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

/** The release's commit or, in development, git's (without git, none). */
function commit(): string {
  if (process.env.APP_COMMIT) return process.env.APP_COMMIT;
  try {
    return execSync("git rev-parse HEAD", { stdio: ["ignore", "pipe", "ignore"] })
      .toString()
      .trim();
  } catch {
    return "";
  }
}

/** The rules editor's and the console's CodeMirror and Lezer packages. */
const CODEMIRROR = [
  "@codemirror/autocomplete",
  "@codemirror/commands",
  "@codemirror/lang-sql",
  "@codemirror/lang-yaml",
  "@codemirror/language",
  "@codemirror/lint",
  "@codemirror/search",
  "@codemirror/state",
  "@codemirror/view",
  "@lezer/highlight",
];

export default defineConfig({
  plugins: [solid()],
  // The release's version and commit (see infra/docker/web.Dockerfile), for
  // the foot of the menu; in development, «dev» and git's commit.
  define: {
    __APP_VERSION__: JSON.stringify(process.env.APP_VERSION || "dev"),
    __APP_COMMIT__: JSON.stringify(commit()),
  },
  server: {
    port: 5173,
    // Behind Traefik in watch mode (`task dev:watch`): the page is
    // https://localhost, so the hot-reload websocket goes through 443.
    hmr: process.env.DEV_HMR_CLIENT_PORT
      ? { clientPort: Number(process.env.DEV_HMR_CLIENT_PORT), protocol: "wss" }
      : undefined,
  },
  build: {
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("index.html", import.meta.url)),
      },
    },
  },
  resolve: {
    // A single Solid runtime instance across the app and every feature package.
    // `@tanstack/solid-query` is deduped too: its context is only shared when
    // every package resolves to the same module instance.
    dedupe: ["solid-js", "@solidjs/router", "@tanstack/solid-query"],
  },
  optimizeDeps: {
    // Feature packages ship Solid source (`.tsx`); keep them out of the esbuild
    // pre-bundle so vite-plugin-solid compiles their JSX with the app's. The
    // query package's `./provider` entry ships `.tsx` for the same reason.
    exclude: [
      "@knime-orchestador/feature-dashboard",
      "@knime-orchestador/feature-users",
      "@knime-orchestador/feature-kit",
      "@knime-orchestador/query",
    ],
    // The rules editor's CodeMirror, pre-bundled together so its packages
    // share one copy of what they import: an extension, a facet or a
    // highlight tag only matches its own module instance.
    // The console's SQL formatter and the texts' i18next too, listed so that
    // the dev server does not discover them on a page's first visit and
    // reload the page to re-bundle.
    include: [
      ...[...CODEMIRROR, "sql-formatter"].map(
        (name) => `@knime-orchestador/feature-quality > ${name}`,
      ),
      "@knime-orchestador/i18n > i18next",
    ],
  },
});
