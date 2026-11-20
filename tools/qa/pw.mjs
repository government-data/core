// A browser session for an interactive test round (docs: development/testing):
// it opens Chromium, signs in, keeps the session between scripts, takes
// screenshots and asks the API with the session's cookie.
//
// Environment:
//   PLAYWRIGHT_CORE  path to playwright-core's index.mjs (default: the package)
//   CHROMIUM         the Chromium to use (default: Playwright's own)
//   BASE             the platform's address (default: https://localhost)
//   QA_ROUND         the round's folder (or pass `dir`)
//   QA_SHOW=1        show the browser while it works (WSLg or a desktop),
//                    with a pause between actions (QA_PAUSE ms, 500 by default)
//   QA_VIDEO=1       record a video of each session in the round's videos/
//   QA_MAILPIT       the test inbox (default http://127.0.0.1:8026, dev's Mailpit)

import fs from "node:fs";
import path from "node:path";

const { chromium } = await import(process.env.PLAYWRIGHT_CORE || "playwright-core");

/**
 * Where a visible browser shows: on WSL, WSLg's Wayland (its socket lives in
 * /mnt/wslg/runtime-dir, not in the usual runtime dir); elsewhere, the
 * desktop's own display.
 */
function display() {
  const wslg = "/mnt/wslg/runtime-dir";
  if (fs.existsSync(path.join(wslg, "wayland-0"))) {
    return {
      args: ["--ozone-platform=wayland"],
      env: { XDG_RUNTIME_DIR: wslg, WAYLAND_DISPLAY: "wayland-0" },
    };
  }
  return { args: [], env: {} };
}

/**
 * Open a session. `dir` is the round's folder: the session state is kept in
 * `dir/state.json` (so each script does not sign in again) and screenshots go
 * to `dir/shots/`.
 */
export async function session({
  dir = process.env.QA_ROUND || ".",
  base = process.env.BASE || "https://localhost",
  headless = process.env.QA_SHOW !== "1",
  slowMo = process.env.QA_SHOW === "1" ? Number(process.env.QA_PAUSE || 500) : 0,
  video = process.env.QA_VIDEO === "1",
  fresh = false,
  viewport = { width: 1400, height: 1000 },
} = {}) {
  const state = path.join(dir, "state.json");
  fs.mkdirSync(path.join(dir, "shots"), { recursive: true });
  const browser = await chromium.launch({
    headless,
    slowMo,
    executablePath: process.env.CHROMIUM || undefined,
    // Visible: full screen, so whoever watches sees it as a user would.
    args: ["--no-sandbox", ...(headless ? [] : ["--start-fullscreen", ...display().args])],
    env: { ...process.env, ...(headless ? {} : display().env) },
  });
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    // Visible, the page takes the window's size.
    viewport: headless ? viewport : null,
    locale: "es-CO",
    ...(!fresh && fs.existsSync(state) ? { storageState: state } : {}),
    ...(video ? { recordVideo: { dir: path.join(dir, "videos"), size: viewport } } : {}),
  });
  const page = await context.newPage();
  // What breaks in the page is evidence too.
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));

  const url = (p) => new URL(p, base).toString();

  return {
    page,
    context,
    browser,
    errors,
    url,

    /** Sign in through the login form and keep the session. */
    async login(email, password) {
      if (!password) throw new Error("missing password (pass it through an environment variable)");
      await page.goto(url("/login"));
      await page.fill('input[type="email"]', email);
      await page.fill('input[type="password"]', password);
      await page.click('button[type="submit"]');
      await page.waitForURL((u) => !u.pathname.startsWith("/login"), { timeout: 20000 });
      await context.storageState({ path: state });
    },

    /** Whether the kept session still works (the API answers 200). */
    async signedIn() {
      await page.goto(url("/"));
      const r = await this.api("/access/me");
      return r.status === 200;
    },

    /** A screenshot in `shots/<name>.png`; returns its path relative to `dir`. */
    async shot(name, { fullPage = false, locator } = {}) {
      const rel = path.join("shots", `${name.replace(/[^\w.-]+/g, "-")}.png`);
      const target = path.join(dir, rel);
      if (locator) await locator.screenshot({ path: target });
      else await page.screenshot({ path: target, fullPage });
      return rel;
    },

    /** Ask the API (`/api` + `p`) with the session: `{ status, body }`. */
    async api(p, { method = "GET", body } = {}) {
      return page.evaluate(
        async ({ p, method, body }) => {
          // As the web does: the CSRF token on everything but GET.
          const csrf = document.cookie.match(/(?:^|; )csrf=([^;]+)/)?.[1];
          const r = await fetch("/api" + p, {
            method,
            credentials: "same-origin",
            headers: {
              ...(body ? { "content-type": "application/json" } : {}),
              ...(method !== "GET" && csrf ? { "x-csrf-token": csrf } : {}),
            },
            body: body ? JSON.stringify(body) : undefined,
          });
          const text = await r.text();
          let parsed = text;
          try {
            parsed = JSON.parse(text);
          } catch {
            // not JSON
          }
          return { status: r.status, body: parsed };
        },
        { p, method, body },
      );
    },

    /** Wait until `check()` returns something truthy (polling), or fail. */
    async until(check, { every = 5000, within = 10 * 60 * 1000, what = "the condition" } = {}) {
      const end = Date.now() + within;
      for (;;) {
        const got = await check();
        if (got) return got;
        if (Date.now() > end) throw new Error(`${what} did not happen within ${within / 1000} s`);
        await page.waitForTimeout(every);
      }
    },

    /**
     * The newest email to `to` in the test inbox (Mailpit), waiting up to
     * `within` ms: `{ subject, text, html, links }` (`links`, the http(s)
     * links it carries, e.g. an activation link).
     */
    async mail(to, { within = 60000, since = 0 } = {}) {
      const inbox = process.env.QA_MAILPIT || "http://127.0.0.1:8026";
      const found = await this.until(
        async () => {
          const r = await fetch(`${inbox}/api/v1/search?query=${encodeURIComponent(`to:"${to}"`)}`);
          if (!r.ok) return null;
          const list = (await r.json()).messages ?? [];
          return list.find((m) => new Date(m.Created).getTime() >= since) ?? null;
        },
        { every: 2000, within, what: `an email to ${to}` },
      );
      const full = await (await fetch(`${inbox}/api/v1/message/${found.ID}`)).json();
      const body = `${full.Text ?? ""}\n${full.HTML ?? ""}`;
      const links = [...new Set(body.match(/https?:\/\/[^\s"'<>)]+/g) ?? [])];
      return { subject: full.Subject, text: full.Text, html: full.HTML, links };
    },

    async close() {
      await browser.close();
    },
  };
}
