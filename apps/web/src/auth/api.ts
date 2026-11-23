/**
 * The auth screens' data layer: the login mutation, the OAuth provider list,
 * and the current-session query. Kept in the shell (not a sidebar feature) —
 * signing in is a precondition for the app, not a slice of it.
 */

import { createApiClient, type components } from "@knime-orchestador/api-client";
import { createFeatureKeys, unwrap } from "@knime-orchestador/query";

/** The authenticated admin's identity (id + role). */
export type Me = components["schemas"]["MeOut"];

/** The auth query-key namespace, shared by the login screen and the guards. */
export const authKeys = createFeatureKeys("auth");

/** The key the current-session query lives under. */
export const meKey = () => authKeys.detail("me");

// One stateless client for the shell's auth calls.
const api = createApiClient();

/**
 * Sign in with an email and password. On success the backend sets the session
 * and CSRF cookies; the resolved value is the admin's id. A failure throws an
 * `ApiError` whose status the caller maps to a *generic* message — the backend
 * never distinguishes "no such account" from "wrong password", and neither does
 * the UI.
 */
export function login(
  email: string,
  password: string,
  captcha?: string | null,
): Promise<components["schemas"]["LoginOk"]> {
  return unwrap(api.POST("/auth/login", { body: { email, password, captcha: captcha ?? null } }));
}

/** The captcha's public key (Turnstile), if this installation uses it. */
export async function captchaSiteKey(): Promise<string | null> {
  try {
    return (await unwrap(api.GET("/auth/captcha"))).site_key ?? null;
  } catch {
    return null;
  }
}

/** Remember the admin's interface language in the account (best-effort: if
 * it is refused, only this browser keeps it). */
export function saveLocale(locale: "es" | "en" | null): Promise<unknown> {
  return api
    .PUT("/auth/me/locale", { body: { locale }, headers: csrfHeader() })
    .catch(() => undefined);
}

/** Log out server-side (best-effort). Sends the CSRF header the mutation needs. */
export function logout(): Promise<unknown> {
  return api.POST("/auth/logout", { headers: csrfHeader() });
}

/**
 * The common account this installation signs in with (`https://cuenta.…`),
 * else `null`: «Sign out» goes through its /signout, and the menu offers
 * switching applications there.
 */
export async function accountUrl(): Promise<string | null> {
  const r = await fetch("/api/settings/account", { credentials: "same-origin" });
  if (!r.ok) return null;
  const { url } = (await r.json()) as { url: string | null };
  return url;
}

/**
 * A place the common account lets this person go (`current`: this one). The
 * names come in Spanish; `kind` lets the menu translate the platform panel.
 */
export interface AccountApp {
  /**
   * `app`: one that signs in with this installation (RustFS…); `module`: an
   * installed module's page on this site (the user guide…), `url` a path.
   */
  kind?: "management" | "instance" | "app" | "module";
  /** Of an `app`: its client id, for its page (`/apps/<id>`). */
  id?: string;
  name: string;
  detail: string;
  url: string;
  /** Its icon (https), for this installation's applications. */
  icon?: string | null;
  current?: boolean;
}

/**
 * Where else this person can go: this installation's applications (RustFS…)
 * and, by the common account, the platform panel and their company's
 * instances.
 */
export async function accountApps(): Promise<AccountApp[]> {
  const r = await fetch("/api/settings/account/apps", { credentials: "same-origin" });
  if (!r.ok) throw new Error(String(r.status));
  const { apps } = (await r.json()) as { apps: AccountApp[] };
  return apps.filter((a) => a.url.startsWith("https://") || isModulePage(a));
}

/** A module's page: a path of this site, never another one. */
export function isModulePage(a: AccountApp): boolean {
  return (
    a.kind === "module" && a.url.startsWith("/") && !a.url.startsWith("//") && !a.url.includes("\\")
  );
}

/** The authenticated admin, or a thrown `ApiError(401)` when no session holds. */
export function getMe(): Promise<Me> {
  return unwrap(api.GET("/auth/me"));
}

/**
 * The enabled OAuth provider ids. Resolves to `[]` when OAuth is disabled (the
 * route is absent, a 404) or otherwise unreachable — the login screen then
 * simply shows no provider buttons.
 */
export async function listProviders(): Promise<string[]> {
  try {
    const out = await unwrap(api.GET("/auth/oauth/providers"));
    return out.providers;
  } catch {
    return [];
  }
}

/** The browser URL that begins an OAuth flow for `provider` (a full redirect);
 * with `next`, the page it goes back to after signing in. */
export function oauthStartUrl(provider: string, next?: string): string {
  const start = `/api/auth/oauth/${encodeURIComponent(provider)}/start`;
  return next ? `${start}?next=${encodeURIComponent(next)}` : start;
}

/** A provider id (`google`) as people read it (`Google`). */
export function providerName(id: string): string {
  return `${id.charAt(0).toUpperCase()}${id.slice(1)}`;
}

/** The password rules a new password must meet. */
export type PasswordPolicy = components["schemas"]["PasswordPolicyOut"];

/**
 * Email a password-reset link. Resolves the same way whether or not the
 * account exists (the backend never says); rejects with `ApiError(503)` when
 * outgoing mail is not configured.
 */
export function requestPasswordReset(
  email: string,
  captcha?: string | null,
): Promise<components["schemas"]["ResetRequestOut"]> {
  return unwrap(
    api.POST("/auth/password-reset/request", { body: { email, captcha: captcha ?? null } }),
  );
}

/** Whether a reset link still works, plus the policy the form shows. */
export function checkPasswordReset(token: string): Promise<components["schemas"]["ResetCheckOut"]> {
  return unwrap(api.POST("/auth/password-reset/check", { body: { token } }));
}

/**
 * Set a new password with a reset link. Rejects with `ApiError(410)` when the
 * link no longer works and `ApiError(422)` when the password is too weak.
 */
export async function confirmPasswordReset(token: string, password: string): Promise<void> {
  await unwrap(api.POST("/auth/password-reset/confirm", { body: { token, password } }));
}

/** The `X-CSRF-Token` header mirrored from the readable `csrf` cookie. */
function csrfHeader(): Record<string, string> {
  const token = document.cookie
    .split("; ")
    .find((row) => row.startsWith("csrf="))
    ?.slice("csrf=".length);
  return token ? { "x-csrf-token": token } : {};
}
