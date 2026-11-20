import { useParams } from "@solidjs/router";
import { lang } from "@knime-orchestador/i18n";
import { createEffect, createResource, onCleanup, Show, type Component } from "solid-js";
import { measureBar, showBar } from "./appFrameBar";
import { tr } from "./i18n";
import "./platform.css";

/** An application that signs in with this installation (`kind: app`). */
interface LocalApp {
  kind?: string;
  id?: string;
  name: string;
  url: string;
}

async function localApps(): Promise<LocalApp[]> {
  const r = await fetch("/api/settings/account/apps", { credentials: "same-origin" });
  if (!r.ok) return [];
  const { apps } = (await r.json()) as { apps: LocalApp[] };
  return apps.filter((a) => a.kind === "app" && a.url.startsWith("https://"));
}

/** The id of the styles put in the application's page. */
const STYLE_ID = "quality-data-merge";

/**
 * In the application's page: its header's right side (language, theme,
 * links and its account's menu) makes room for Quality Data's avatar, laid
 * over it from this page; and while Quality Data changes its language
 * through its own menu (`qd-switching`), that menu is not seen.
 */
/**
 * The application's top bar. Not every `header` of its page: since RustFS
 * 1.0.5 each page's title and actions («Add Access Key») are a
 * `header.page-header` too, and must stay.
 */
const TOP_BAR = "header:not(.page-header)";

const MERGE_CSS = `
${TOP_BAR} > :last-child { visibility: hidden !important; }
.qd-switching [role="menu"],
.qd-switching [data-radix-popper-content-wrapper],
.qd-switching [data-floating-ui-portal] { opacity: 0 !important; }
`;

/**
 * In Grafana's page: only its profile menu is hidden (the account is
 * Quality Data's), and its top bar keeps room on the right for the avatar.
 */
const GRAFANA_CSS = `
header > :first-child { padding-right: 10rem !important; }
header button[aria-label="Profile"],
header button[aria-label="Perfil"] { visibility: hidden !important; }
`;

/**
 * In Trino's page (its web UI, in English only): with Quality Data's light
 * theme its navy top bar turns white, with dark ink; its theme button and its
 * own avatar go (both are Quality Data's), and the bar keeps room on the
 * right for Quality Data's avatar.
 */
const TRINO_CSS = `
html.qd-light .MuiAppBar-root {
  background-color: #fff !important;
  color: #1c1c22 !important;
  box-shadow: 0 1px 0 rgba(0, 0, 0, 0.08) !important;
}
.MuiAppBar-root button[aria-label="Themes"],
.MuiAppBar-root button:has(.MuiAvatar-root) { display: none !important; }
.MuiAppBar-root .MuiToolbar-root { padding-right: 10rem !important; }
`;

/** Where Trino's UI keeps its settings: its store, in this domain's storage. */
const TRINO_STORE = "app-config";

/**
 * Trino follows Quality Data's theme: its store's `theme` is set to it and
 * its page reloaded, only when it changed (its store is read on start).
 */
function syncTrino(frame: HTMLIFrameElement) {
  const root = frame.contentDocument?.documentElement;
  root?.classList.remove("qd-light", "qd-dark");
  root?.classList.add(`qd-${currentTheme()}`);
  try {
    const raw = localStorage.getItem(TRINO_STORE);
    const store = (raw ? JSON.parse(raw) : { state: {}, version: 1 }) as {
      state?: { theme?: string };
    };
    if (store.state?.theme === currentTheme()) return;
    store.state = { ...store.state, theme: currentTheme() };
    localStorage.setItem(TRINO_STORE, JSON.stringify(store));
    frame.contentWindow?.location.reload();
  } catch {
    // Without storage Trino keeps its own.
  }
}

/** How the application names each language in its own menu (RustFS). */
const LANGUAGE_NAMES: Record<string, string> = { es: "Español", en: "English" };

/** Quality Data's theme now: the one chosen, or the system's. */
function currentTheme(): "light" | "dark" {
  const chosen = document.documentElement.dataset.theme;
  if (chosen === "light" || chosen === "dark") return chosen;
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * Quality Data's language and theme, where the application reads them on
 * start (RustFS: i18next's `lang` / `i18n_redirected` and next-themes'
 * `active_theme`). It is on this same domain, so it shares this storage.
 */
function shareSettings() {
  try {
    localStorage.setItem("lang", lang());
    localStorage.setItem("i18n_redirected", lang());
    localStorage.setItem("active_theme", currentTheme());
  } catch {
    // Without storage the application keeps its own.
  }
}

/** The application's page in Quality Data's theme, at once (RustFS: a class
 * on <html>, which its styles follow). */
function applyTheme(frame: HTMLIFrameElement) {
  const doc = frame.contentDocument;
  if (!doc) return;
  const theme = currentTheme();
  doc.documentElement.classList.remove("light", "dark");
  doc.documentElement.classList.add(theme);
  doc.documentElement.style.colorScheme = theme;
}

/** How Grafana names Quality Data's languages in its preferences. */
const GRAFANA_LANGUAGES: Record<string, string> = { es: "es-ES", en: "en-US" };

/**
 * Grafana keeps its theme and language as the user's preferences, on its
 * server, not in the page: they are set through its API (on this same
 * domain, with its session's cookie) and its page reloaded, only when they
 * changed. Before its sign-in ends there is no session: the sign-in finishes
 * in another load of its page, which syncs again.
 */
async function syncGrafana(frame: HTMLIFrameElement) {
  const url = new URL("api/user/preferences", frame.src).pathname;
  const want = { theme: currentTheme(), language: GRAFANA_LANGUAGES[lang()] ?? "en-US" };
  // Each theme and language once per frame: what Grafana answers back is not
  // always what it was sent, and comparing it would reload without end.
  const key = `${want.theme}|${want.language}`;
  if (frame.dataset.grafanaPrefs === key) return;
  try {
    const now = await fetch(url, { credentials: "same-origin" });
    if (!now.ok) return;
    const { theme, language } = (await now.json()) as { theme?: string; language?: string };
    frame.dataset.grafanaPrefs = key;
    if (theme === want.theme && language === want.language) return;
    const saved = await fetch(url, {
      method: "PATCH",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(want),
    });
    if (saved.ok) frame.contentWindow?.location.reload();
  } catch {
    // Unreachable for now: Grafana keeps its own until the next load.
  }
}

/**
 * How each application follows Quality Data's theme and language. RustFS
 * reads them from this domain's storage and a class on its page, so it
 * changes without reloading; Grafana keeps them on its server.
 */
interface Adapter {
  /** The styles put in its page. */
  css: string;
  /** Its top bar, over which the avatar is centered. */
  bar: string;
  /** Whether it speaks Quality Data's languages (else the menu hides them). */
  translated: boolean;
  theme(frame: HTMLIFrameElement): void;
  language(frame: HTMLIFrameElement, next: string): Promise<void>;
}

const ADAPTERS: Record<string, Adapter> = {
  grafana: {
    css: GRAFANA_CSS,
    bar: "header > :first-child",
    translated: true,
    theme: (frame) => void syncGrafana(frame),
    language: (frame) => syncGrafana(frame),
  },
  trino: {
    css: TRINO_CSS,
    bar: ".MuiAppBar-root",
    translated: false,
    theme: syncTrino,
    language: async () => {},
  },
};

const RUSTFS_LIKE: Adapter = {
  css: MERGE_CSS,
  bar: TOP_BAR,
  translated: true,
  theme: applyTheme,
  language: switchLanguage,
};

/** The adapter of each open frame. */
const adapters = new WeakMap<HTMLIFrameElement, Adapter>();

const adapterOf = (frame: HTMLIFrameElement) => adapters.get(frame) ?? RUSTFS_LIKE;

/**
 * Ready the application's page to sit inside Quality Data: its header's
 * right side makes room for Quality Data's avatar (the way back is in its
 * menu), and its theme is Quality Data's. The application is on this same
 * domain, so its page can be reached from here.
 */
function prepare(frame: HTMLIFrameElement) {
  const doc = frame.contentDocument;
  if (!doc?.head) return;
  const adapter = adapterOf(frame);
  if (!doc.getElementById(STYLE_ID)) {
    const style = doc.createElement("style");
    style.id = STYLE_ID;
    style.textContent = adapter.css;
    doc.head.append(style);
  }
  adapter.theme(frame);
  frame.dataset.lang = lang();
  void measureBar(frame, adapter.bar);
}

/**
 * When Quality Data's theme changes (chosen in the avatar's menu, or the
 * system's), every open application follows.
 */
function followTheme() {
  const update = () => {
    shareSettings();
    for (const frame of frames.values()) {
      adapterOf(frame).theme(frame);
      // Its bar's background changed with the theme: the name's ink too.
      setTimeout(() => void measureBar(frame, adapterOf(frame).bar), 300);
    }
  };
  new MutationObserver(update).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", update);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Change the application's language without reloading it, as a person
 * would: its own language menu, opened and chosen out of sight. If it is not
 * there (another version), its page is reloaded with the language already
 * in its storage.
 */
async function switchLanguage(frame: HTMLIFrameElement, next: string) {
  const win = frame.contentWindow;
  const doc = frame.contentDocument;
  const name = LANGUAGE_NAMES[next];
  // Its first button on the right of its header (named in its own language:
  // «Language», «Idioma»…).
  const button = doc?.querySelector<HTMLElement>(`${TOP_BAR} > :last-child button`);
  if (!win || !doc || !name || !button) {
    win?.location.reload();
    return;
  }
  // Its own event classes: the application's code is of its page.
  const W = win as unknown as typeof globalThis;
  doc.documentElement.classList.add("qd-switching");
  try {
    const press = { bubbles: true, cancelable: true, button: 0, view: win };
    button.dispatchEvent(new W.PointerEvent("pointerdown", { ...press, pointerType: "mouse" }));
    button.dispatchEvent(new W.MouseEvent("mousedown", press));
    button.dispatchEvent(new W.PointerEvent("pointerup", { ...press, pointerType: "mouse" }));
    button.dispatchEvent(new W.MouseEvent("click", press));
    let item: HTMLElement | undefined;
    for (let i = 0; i < 20 && !item; i++) {
      await sleep(50);
      item = [...doc.querySelectorAll<HTMLElement>('[role^="menuitem"], [role="option"]')].find(
        (el) => el.textContent?.trim() === name,
      );
    }
    if (!item) {
      win.location.reload();
      return;
    }
    item.dispatchEvent(new W.PointerEvent("pointerdown", { ...press, pointerType: "mouse" }));
    item.click();
    frame.dataset.lang = next;
  } finally {
    await sleep(100);
    doc.documentElement.classList.remove("qd-switching");
  }
}

/**
 * The applications' frames, kept out of the app's tree: changing the language
 * builds the app again, and a frame that left the page would load again. They
 * live in a layer of their own under the avatar, shown while their page is
 * open; leaving and coming back finds the application where it was.
 */
const frames = new Map<string, HTMLIFrameElement>();

function layer(): HTMLElement {
  let host = document.getElementById("app-frames");
  if (!host) {
    host = document.createElement("div");
    host.id = "app-frames";
    document.body.append(host);
    followTheme();
  }
  return host;
}

function frameFor(app: LocalApp): HTMLIFrameElement {
  const key = app.id ?? app.url;
  let frame = frames.get(key);
  if (!frame) {
    shareSettings();
    frame = document.createElement("iframe");
    frame.className = "app-frame";
    frame.title = app.name;
    frame.src = app.url;
    const adapter = app.id ? ADAPTERS[app.id] : undefined;
    if (adapter) adapters.set(frame, adapter);
    const f = frame;
    f.addEventListener("load", () => prepare(f));
    layer().append(f);
    frames.set(key, f);
  }
  return frame;
}

/**
 * An application of this installation (RustFS…) inside Quality Data, as if it
 * were one of its pages: it fills the page, with Quality Data's avatar over
 * its header; the way back is in the avatar's menu. It speaks Quality Data's
 * language and changes with it, without reloading. Its address signs in with
 * Quality Data by itself, so it opens without its own form.
 */
export const AppFrame: Component = () => {
  const params = useParams<{ id: string }>();
  const [apps] = createResource(localApps);
  const app = () => apps()?.find((a) => a.id === params.id);

  // While it is open: of Quality Data, only the avatar stays (over the
  // application's header), and the application's frame shows.
  createEffect(() => {
    const a = app();
    if (!a) return;
    const frame = frameFor(a);
    document.body.classList.add("app-frame-open");
    frame.hidden = false;
    showBar(frame);
    document.body.classList.toggle("app-frame-no-lang", !adapterOf(frame).translated);
    // Another language since it loaded: the application follows.
    if (frame.dataset.lang && frame.dataset.lang !== lang()) {
      shareSettings();
      void adapterOf(frame).language(frame, lang());
    }
    onCleanup(() => {
      frame.hidden = true;
      document.body.classList.remove("app-frame-open");
      showBar();
      document.body.classList.remove("app-frame-no-lang");
    });
  });

  return (
    <Show when={apps.state === "ready" && !app()}>
      <p class="table__muted">{tr("platform:apps.unknown")}</p>
    </Show>
  );
};
