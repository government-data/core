/**
 * The data layer of the system settings (`/settings/*`, admin only).
 *
 * Secrets never arrive: the field comes empty and `has_<field>` says whether
 * one is saved. On save, an empty secret keeps the current one.
 */
import { csrfHeader, request, SettingsApiError } from "./http";
import { t } from "./i18n";

export { SettingsApiError } from "./http";
export * from "./iamApi";

export type {
  AllSettings,
  LdapSettings,
  MailSettings,
  MailTransport,
  OAuthProvider,
  OAuthSettings,
  PlatformSettings,
  Section,
  SmtpSecurity,
  Source,
} from "./sectionTypes";
import type {
  AllSettings,
  LdapSettings,
  MailSettings,
  OAuthSettings,
  PlatformSettings,
  Section,
} from "./sectionTypes";

export interface LdapProbe {
  searched: boolean;
  user: { dn: string; email: string | null; display_name: string | null; groups: string[] } | null;
}

export interface OidcDiscovery {
  issuer: string;
  auth_endpoint: string;
  token_endpoint: string;
  userinfo_endpoint: string;
}

export const settingsApi = {
  all: () => request<AllSettings>("GET", "/settings"),

  saveLdap: (s: LdapSettings) => request<Section<LdapSettings>>("PUT", "/settings/ldap", s),
  testLdap: (settings: LdapSettings, login: string) =>
    request<LdapProbe>("POST", "/settings/ldap/test", { settings, login }),

  saveMail: (s: MailSettings) => request<Section<MailSettings>>("PUT", "/settings/mail", s),
  testMail: (settings: MailSettings, to: string) =>
    request<void>("POST", "/settings/mail/test", { settings, to }),

  saveOAuth: (s: OAuthSettings) => request<Section<OAuthSettings>>("PUT", "/settings/oauth", s),
  discover: (issuer: string) =>
    request<OidcDiscovery>("POST", "/settings/oauth/discover", { issuer }),

  savePlatform: (s: PlatformSettings) =>
    request<Section<PlatformSettings>>("PUT", "/settings/platform", s),

  /** Forget what is saved and go back to the environment variables. */
  reset: (section: "ldap" | "mail" | "oauth" | "platform") =>
    request<AllSettings>("DELETE", `/settings/${section}`),
};

/** An account of another provider linked to yours. */
export interface LinkedIdentity {
  provider: string;
  subject: string;
  email?: string | null;
  /** Seconds since 1970. */
  linked_at: number;
}

/** A provider id (`google`) as it reads (`Google`). */
export const providerName = (id: string) => `${id.charAt(0).toUpperCase()}${id.slice(1)}`;

/** «Cuentas vinculadas»: what belongs to the signed-in person, whatever their role. */
export const accountApi = {
  /** The configured providers; `[]` without OAuth (the route does not exist). */
  providers: async (): Promise<string[]> => {
    try {
      const res = await fetch("/api/auth/oauth/providers", { credentials: "same-origin" });
      if (!res.ok) return [];
      return ((await res.json()) as { providers: string[] }).providers;
    } catch {
      return [];
    }
  },
  identities: () =>
    request<{ identities: LinkedIdentity[] }>("GET", "/auth/me/identities").then(
      (out) => out.identities,
    ),
  unlink: (i: LinkedIdentity) =>
    request<void>(
      "DELETE",
      `/auth/me/identities/${encodeURIComponent(i.provider)}/${encodeURIComponent(i.subject)}`,
    ),
  /** Upload the person's own profile photo (the image itself as the body). */
  uploadPhoto: async (file: Blob): Promise<void> => {
    const res = await fetch("/api/auth/me/avatar", {
      method: "PUT",
      credentials: "same-origin",
      headers: { ...csrfHeader(), "content-type": file.type || "application/octet-stream" },
      body: file,
    });
    if (!res.ok) {
      const body = (await res.json().catch(() => ({}))) as { message?: string };
      throw new SettingsApiError(
        res.status,
        body.message ?? t("settings:api.error", { status: res.status }),
      );
    }
  },
  /** Remove it: the initial shows (or the provider's photo, at the next sign-in). */
  removePhoto: () => request<void>("DELETE", "/auth/me/avatar"),
  /** Go to the provider to link an account (comes back to /settings/account). */
  linkUrl: (provider: string) => `/api/auth/oauth/${encodeURIComponent(provider)}/link`,
};

/** An account in Settings → Users. */
export interface UserAccount {
  id: string;
  email: string;
  display_name?: string | null;
  role: string;
  /** `active` (has a password), `pending` (still has to activate it) or
   * `directory` (signs in with LDAP / AD). */
  status: "active" | "pending" | "directory";
  /** Seconds since 1970. */
  created_at: number;
}

export interface UsersList {
  users: UserAccount[];
  /** The roles a new account can have. */
  roles: string[];
  /** Without outgoing mail nobody can be created (the link would not reach them). */
  mail_configured: boolean;
}

/** The users (`admin:ManageUsers`). Creating one sends them their activation link. */
export const usersApi = {
  list: () => request<UsersList>("GET", "/users"),
  create: (u: { email: string; display_name: string | null; role: string }) =>
    request<{ id: string; activation_sent: boolean }>("POST", "/users", u),
  resend: (id: string) => request<void>("POST", `/users/${encodeURIComponent(id)}/activation`),
  update: (id: string, u: { display_name: string | null; role: string }) =>
    request<void>("PATCH", `/users/${encodeURIComponent(id)}`, u),
  remove: (id: string) => request<void>("DELETE", `/users/${encodeURIComponent(id)}`),
};

/** Who I am and what I may do (ADR 0016): the actions on some resource. */
export interface Me {
  admin_id: string;
  is_admin: boolean;
  service_account?: string | null;
  actions: string[];
}

export const meApi = () => request<Me>("GET", "/access/me");

/** Whether `me` may do `action` (on some resource); `admin`, everything. */
export function may(me: Me | undefined, action: string): boolean {
  return !!me && (me.is_admin || me.actions.includes(action));
}

/** A global secret. The value never arrives. */
export interface GlobalSecret {
  id: string;
  name: string;
  description: string;
  updated_at: string;
  /** `value` (a single value) or `database` (a connection). */
  kind: "value" | "database";
  /** Of a connection, what is not secret and which of its secret fields it has. */
  meta: { engine?: string; fields?: Record<string, string>; secrets?: string[] };
}

/** The global secrets (`/secrets`): those one may manage (`admin:ManageSecrets`). */
export const secretsApi = {
  list: () => request<GlobalSecret[]>("GET", "/secrets"),
  /** `value`: a text, or a connection (`connectionValue` of connections.ts). */
  create: (body: { name: string; description: string; value: unknown }) =>
    request<GlobalSecret>("POST", "/secrets", body),
  update: (id: string, body: { description?: string; value?: unknown }) =>
    request<GlobalSecret>("PATCH", `/secrets/${id}`, body),
  remove: (id: string) => request<void>("DELETE", `/secrets/${id}`),
};
