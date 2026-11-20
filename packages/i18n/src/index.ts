/**
 * The UI languages (i18n): Spanish and English, with i18next.
 *
 * Each package brings its texts (`src/locales/es.json` and `en.json`) and
 * registers them with {@link registerLocales} under its namespace (`engine`,
 * `settings`…); the screens read them with {@link t} (`t("engine:updates.save")`).
 * The active language is a Solid signal: changing it repaints everything
 * without reloading. It comes from the preference saved in the account, and
 * if there is none, from the browser (Spanish unless it is English). Dates,
 * numbers and sizes, with `Intl` in the active language.
 */
import i18next from "i18next";
import { createSignal } from "solid-js";

/** The UI languages. */
export const LANGS = ["es", "en"] as const;
export type Lang = (typeof LANGS)[number];

/** The default language, and the fallback when a translation is missing. */
export const DEFAULT_LANG: Lang = "es";

/** Where this browser remembers it (before signing in, or without an account). */
const STORAGE_KEY = "orq.lang";

/** A dictionary: nested keys down to a text. */
export type Messages = { [key: string]: string | Messages };

export function isLang(value: unknown): value is Lang {
  return typeof value === "string" && (LANGS as readonly string[]).includes(value);
}

/** The language of a list like the browser's (`["en-US", "es"]`). */
export function pickLang(preferred: readonly string[]): Lang {
  for (const tag of preferred) {
    const base = tag.toLowerCase().split("-")[0];
    if (isLang(base)) return base;
  }
  return DEFAULT_LANG;
}

function stored(): Lang | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return isLang(v) ? v : null;
  } catch {
    return null;
  }
}

function detect(): Lang {
  const saved = stored();
  if (saved) return saved;
  if (typeof navigator !== "undefined") {
    return pickLang(navigator.languages?.length ? navigator.languages : [navigator.language]);
  }
  return DEFAULT_LANG;
}

const [current, setCurrent] = createSignal<Lang>(detect());

void i18next.init({
  lng: current(),
  fallbackLng: DEFAULT_LANG,
  supportedLngs: [...LANGS],
  // The texts ship in the bundle: nothing to wait for.
  initAsync: false,
  resources: {},
  interpolation: { escapeValue: false },
  returnNull: false,
});

/** The active language (reactive). */
export const lang = current;

/** Changes the UI language (and remembers it in this browser). */
export function setLang(next: Lang): void {
  if (!isLang(next)) return;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // Without storage, it lasts until a reload.
  }
  void i18next.changeLanguage(next);
  if (typeof document !== "undefined") document.documentElement.lang = next;
  setCurrent(next);
}

/** Registers a package's texts under its namespace. */
export function registerLocales(ns: string, locales: Record<Lang, Messages>): void {
  for (const l of LANGS) {
    i18next.addResourceBundle(l, ns, locales[l], true, true);
  }
}

/**
 * A key's text (`"engine:updates.save"`) in the active language, with its
 * values (`{ count: 3 }` picks the plural). Reactive inside JSX.
 */
export function t(key: string, values?: Record<string, unknown>): string {
  current();
  return i18next.t(key, (values ?? {}) as Record<string, string>);
}

/**
 * A value made of texts that a module builds once (a map of labels, a list of
 * options) and that follows the language: it is built again the first time
 * it is read after the language changes, and reading it in JSX tracks the
 * language. `build` must only read texts.
 */
export function translated<T extends object>(build: () => T): T {
  let built: T = build();
  let at: Lang = current();
  const now = (): T => {
    const l = current();
    if (l !== at) {
      built = build();
      at = l;
    }
    return built;
  };
  const target = (Array.isArray(built) ? [] : {}) as T;
  return new Proxy(target, {
    get(_, key) {
      const value = now();
      const got = Reflect.get(value, key) as unknown;
      // An array's methods run on the built array.
      return typeof got === "function" ? (got as (...a: unknown[]) => unknown).bind(value) : got;
    },
    has: (_, key) => Reflect.has(now(), key),
    ownKeys: () => Reflect.ownKeys(now()),
    getOwnPropertyDescriptor(_, key) {
      const d = Reflect.getOwnPropertyDescriptor(now(), key);
      // As the target's own `length` (an array), or configurable.
      return d && key !== "length" ? { ...d, configurable: true } : d;
    },
  });
}

/** Whether a key exists (for optional texts). */
export function exists(key: string): boolean {
  current();
  return i18next.exists(key);
}

/** The BCP 47 tag for `Intl` (es-CO, en-US). */
export function locale(l: Lang = current()): string {
  return l === "es" ? "es-CO" : "en-US";
}

/** Date and time in the active language. */
export function formatDateTime(
  value: string | number | Date,
  options: Intl.DateTimeFormatOptions = { dateStyle: "medium", timeStyle: "short" },
): string {
  const d = value instanceof Date ? value : new Date(value);
  return Number.isNaN(d.getTime()) ? "" : new Intl.DateTimeFormat(locale(), options).format(d);
}

/** A number in the active language. */
export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(locale(), options).format(value);
}

/** How long ago (or how long until), in words: «hace 5 minutos», «5 minutes ago». */
export function formatRelative(value: string | number | Date, now: Date = new Date()): string {
  const d = value instanceof Date ? value : new Date(value);
  const secs = Math.round((d.getTime() - now.getTime()) / 1000);
  const rtf = new Intl.RelativeTimeFormat(locale(), { numeric: "auto" });
  const abs = Math.abs(secs);
  if (abs < 60) return rtf.format(secs, "second");
  if (abs < 3600) return rtf.format(Math.round(secs / 60), "minute");
  if (abs < 86400) return rtf.format(Math.round(secs / 3600), "hour");
  return rtf.format(Math.round(secs / 86400), "day");
}

/** A dictionary's keys, flattened (`servers.title`), for the tests. */
export function flatKeys(messages: Messages, prefix = ""): string[] {
  return Object.entries(messages).flatMap(([k, v]) =>
    typeof v === "string" ? [prefix + k] : flatKeys(v, `${prefix}${k}.`),
  );
}

/**
 * Sends the chosen language on every API call (`Accept-Language`), so the
 * server answers its messages in it. Installed once at startup.
 */
export function sendLanguageToApi(prefix = "/api"): void {
  if (typeof window === "undefined" || !window.fetch) return;
  const original = window.fetch.bind(window);
  window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.pathname : input.url;
    const own = url.startsWith(prefix) || url.startsWith(`${location.origin}${prefix}`);
    if (!own) return original(input, init);
    const headers = new Headers(
      init?.headers ?? (input instanceof Request ? input.headers : undefined),
    );
    headers.set("Accept-Language", current());
    return original(input, { ...init, headers });
  };
}
