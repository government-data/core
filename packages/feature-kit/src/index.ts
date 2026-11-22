/**
 * Feature contract shared by the app shell and every feature package.
 *
 * A *feature* is a workspace package that owns a slice of the admin app: the
 * routes it renders and the sidebar entries that link to them. The shell knows
 * nothing about any particular feature — it only consumes {@link FeatureModule}
 * descriptors. Adding a feature therefore means publishing a new package and
 * registering its module with the shell; no existing feature is touched.
 */

import { createContext, useContext, type Component } from "solid-js";
import type { RouteDefinition } from "@solidjs/router";

/** A single entry rendered in the admin sidebar navigation. */
export interface NavItem {
  /**
   * Absolute path the entry links to. Should match one of the feature's
   * {@link FeatureModule.routes} so the link resolves within the shell.
   */
  path: string;
  /** Text shown in the sidebar. */
  label: string;
  /**
   * Optional leading glyph: a short symbol, or a component that draws it (an
   * SVG that takes the text color), rendered at `size` pixels.
   */
  icon?: string | Component<{ size?: number }>;
  /**
   * Sort weight within the sidebar; lower values come first. Entries without a
   * weight fall back to {@link DEFAULT_NAV_ORDER}.
   */
  order?: number;
  /**
   * Optional extra condition, read reactively: the entry shows only while it
   * returns true (e.g. a per-area permission the role alone does not tell).
   */
  visible?: () => boolean;
  /**
   * Roles allowed to see this nav entry. When absent or empty the entry is
   * visible to every authenticated user; when set, only users whose role is
   * listed will see it.
   */
  roles?: string[];
}

/**
 * The public surface a feature package exposes to the shell: the routes it
 * contributes to the router and the navigation entries that reach them.
 */
export interface FeatureModule {
  /** Stable, unique identifier for the feature (e.g. `"dashboard"`). */
  id: string;
  /** Human-readable title, used for labelling and diagnostics. */
  title: string;
  /** Routes mounted by the shell under the admin layout. */
  routes: RouteDefinition[];
  /** Sidebar entries the feature contributes. Optional for headless features. */
  nav?: NavItem[];
  /**
   * Optional component the shell mounts once, next to the content and outside
   * any route, for UI that must stay visible across sections (notifications,
   * the progress of a background task). Subject to {@link FeatureModule.roles}.
   */
  overlay?: Component;
  /**
   * Roles allowed to access this feature's routes and nav entries. When absent
   * or empty every authenticated user has access; when set, the shell filters
   * navigation and guards routes for users whose role is not listed.
   */
  roles?: string[];
  /** Cards this feature adds to Configuración's home. */
  settingsCards?: SettingsCard[];
  /**
   * A panel the shell shows in the sidebar, under the entries, on every
   * page (a tree to browse, as a database client shows its servers).
   * Subject to {@link FeatureModule.roles}.
   */
  panel?: SidebarPanel;
}

/** A feature's panel in the sidebar: it takes the room left under the entries and scrolls. */
export interface SidebarPanel {
  /** What it holds, for assistive technology. */
  label: string;
  component: Component;
  /** Whether the user sees it (e.g. the actions they may do); always, if missing. */
  visible?: () => boolean;
}

/** A section a feature adds to Configuración: a card on its home that leads
 * to the feature's page. */
export interface SettingsCard {
  path: string;
  label: string;
  description: string;
  icon?: Component<{ size?: number }>;
  /** Whether the user sees it (e.g. the actions they may do); always, if missing. */
  visible?: () => boolean;
}

/** Every feature's settings cards, provided by the shell. */
export const SettingsCardsContext = createContext<SettingsCard[]>([]);

/** The settings cards the registered features contribute. */
export function useSettingsCards(): SettingsCard[] {
  return useContext(SettingsCardsContext);
}

/** Fallback sort weight for a {@link NavItem} that omits `order`. */
export const DEFAULT_NAV_ORDER = 100;

/**
 * Identity helper for authoring a {@link FeatureModule}. It adds no behaviour —
 * it exists to give feature authors inference and a single, greppable choke
 * point the shell contract flows through.
 */
export function defineFeature(feature: FeatureModule): FeatureModule {
  return feature;
}

/**
 * Check whether a user role is allowed by the given role list. Returns `true`
 * when `allowedRoles` is `undefined` or empty (open to all authenticated
 * users), or when `userRole` is included in the array.
 */
export function isRoleAllowed(userRole: string, allowedRoles?: string[]): boolean {
  return !allowedRoles || allowedRoles.length === 0 || allowedRoles.includes(userRole);
}

// ---------------------------------------------------------------------------
// Signed-out pages and an expired session.
// ---------------------------------------------------------------------------

/**
 * The pages that work without a session: an API call there that answers 401
 * is expected, never a reason to leave the page (a link from an activation or
 * reset email has to show its form).
 */
export const PUBLIC_PATHS = ["/login", "/forgot-password", "/reset-password", "/activate"];

/** Whether `pathname` is one of the {@link PUBLIC_PATHS}. */
export function isPublicPath(pathname: string): boolean {
  return PUBLIC_PATHS.includes(pathname);
}

/** What the features forget when the session ends, registered with {@link onSignOut}. */
const forgetting: (() => void)[] = [];

/**
 * Registers what a feature forgets when the session ends: what it kept in
 * the tab for the person (decision 0069's reasons for a view).
 */
export function onSignOut(forget: () => void): void {
  forgetting.push(forget);
}

/** The session ends (signing out, or it expired): every feature forgets what it kept. */
export function signedOut(): void {
  for (const forget of forgetting) forget();
}

/**
 * A feature's API answered 401: the session is gone. What the features
 * kept is forgotten and, off the public pages, the browser goes to sign in
 * and comes back here afterwards.
 */
export function sessionExpired(): void {
  signedOut();
  const { pathname, search } = window.location;
  if (isPublicPath(pathname)) return;
  window.location.assign(`/login?next=${encodeURIComponent(pathname + search)}`);
}

// ---------------------------------------------------------------------------
// Session context — shared between the shell and every feature package.
// ---------------------------------------------------------------------------

/** Minimal session shape feature packages may depend on. */
export interface Session {
  admin_id: string;
  role: string;
  email?: string;
  display_name?: string | null;
  /** The server's version and build commit (for debugging, in the shell). */
  version?: string;
  commit?: string | null;
  /** The interface language the admin chose (`es` / `en`), if any. */
  locale?: string | null;
  /** The profile picture's version (`/auth/me/avatar?v=…`), if it has one. */
  avatar?: number | null;
}

/**
 * The event a feature dispatches on `window` after changing the person's own
 * profile (their photo): the shell asks for the session again.
 */
export const PROFILE_CHANGED = "orq:profile-changed";

/**
 * Context provided by `<RequireSession>` in the shell. Feature packages should
 * never create or provide this context — they only consume it via
 * {@link useSession} or {@link useHasRole}.
 */
export const SessionContext = createContext<() => Session>();

/**
 * The current admin's identity (id + role). Throws if used outside a
 * `<RequireSession>` subtree — a programming error, since the guard guarantees
 * a session before rendering its children.
 */
export function useSession(): Session {
  const session = useContext(SessionContext);
  if (!session) {
    throw new Error("useSession must be used within <RequireSession>");
  }
  // Each read goes to the current session: when it is read again (a new
  // photo), what shows it follows.
  return new Proxy({} as Session, {
    get: (_, key) => session()[key as keyof Session],
    has: (_, key) => key in session(),
    ownKeys: () => Reflect.ownKeys(session()),
    getOwnPropertyDescriptor: (_, key) => {
      const d = Reflect.getOwnPropertyDescriptor(session(), key);
      return d && { ...d, configurable: true };
    },
  });
}

/**
 * Convenience hook that checks whether the authenticated user has one of the
 * given roles. Uses the session already resolved by `<RequireSession>` — no
 * extra fetch required.
 */
export function useHasRole(...roles: string[]): boolean {
  const session = useSession();
  return isRoleAllowed(session.role, roles);
}
