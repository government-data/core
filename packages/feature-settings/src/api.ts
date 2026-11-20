/**
 * The data layer of the system settings (`/settings/*`, admin only).
 *
 * Secrets never arrive: the field comes empty and `has_<field>` says whether
 * one is saved. On save, an empty secret keeps the current one.
 */
import { t } from "./i18n";
import { sessionExpired } from "@knime-orchestador/feature-kit";

export interface Source {
  kind: "database" | "environment" | "unset";
  updated_at: string | null;
  updated_by: string | null;
}

export interface Section<T> {
  value: T;
  source: Source;
}

export interface LdapSettings {
  enabled: boolean;
  url: string;
  starttls: boolean;
  tls_insecure: boolean;
  bind_dn: string;
  bind_password: string;
  has_bind_password?: boolean;
  base_dn: string;
  user_filter: string;
  admin_groups: string[];
  timeout_secs: number;
}

export type MailTransport = "smtp" | "graph";
export type SmtpSecurity = "none" | "starttls" | "tls";

export interface MailSettings {
  enabled: boolean;
  transport: MailTransport;
  from_address: string;
  from_name: string;
  smtp_host: string;
  smtp_port: number;
  smtp_security: SmtpSecurity;
  smtp_username: string;
  smtp_password: string;
  has_smtp_password?: boolean;
  graph_tenant_id: string;
  graph_client_id: string;
  graph_client_secret: string;
  has_graph_client_secret?: boolean;
  /** Minutes the password reset link is valid; 0, the environment's. */
  reset_link_minutes: number;
  /** The language of the email notices (`es` / `en`); empty: Spanish. */
  notice_language?: string;
  /** Set by the platform (online): the server is not shown or changed here. */
  platform?: boolean;
}

export interface OAuthProvider {
  id: string;
  enabled: boolean;
  client_id: string;
  client_secret: string;
  has_client_secret?: boolean;
  issuer: string;
  auth_endpoint: string;
  token_endpoint: string;
  userinfo_endpoint: string;
  scopes: string[];
  /** Link by email even if the provider does not send `email_verified`. */
  trust_email: boolean;
  /** Read-only: the URL to register with the provider. */
  redirect_uri?: string;
}

export interface OAuthSettings {
  providers: OAuthProvider[];
}

export interface AllSettings {
  ldap: Section<LdapSettings>;
  mail: Section<MailSettings>;
  oauth: Section<OAuthSettings>;
  /** The redirect URL with `{id}` in place of the provider's id. */
  oauth_redirect_template: string;
  /** Minutes the recovery link is valid when the mail settings do not set others. */
  reset_link_default_minutes: number;
  /** Where the key that encrypts the secrets comes from. */
  encryption_key: "environment" | "file" | "generated";
  /** The platform's sender for whoever does not configure their own mail
   * (online); `null` if there is none. */
  platform_mail_from?: string | null;
  /** The common account the instance signs in with; `null` if there is none.
   * With it OAuth2 is not configured here: gestión handles the access. */
  platform_account?: string | null;
}

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

/** An API error with its message and, if it is a validation one, the fields. */
export class SettingsApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public fields: Record<string, string> = {},
    /** The API's stable code (`email_taken`, `last_admin`…). */
    public code = "",
  ) {
    super(message);
  }
}

function csrfHeader(): Record<string, string> {
  const token = document.cookie
    .split("; ")
    .find((row) => row.startsWith("csrf="))
    ?.slice("csrf=".length);
  return token ? { "x-csrf-token": token } : {};
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    credentials: "same-origin",
    headers: {
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...(method === "GET" ? {} : csrfHeader()),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.status === 401) {
    sessionExpired();
    throw new SettingsApiError(401, t("settings:api.expired"));
  }
  if (!response.ok) {
    let message = t("settings:api.error", { status: response.status });
    let fields: Record<string, string> = {};
    let code = "";
    try {
      const payload = (await response.json()) as {
        code?: string;
        message?: string;
        fields?: Record<string, string>;
      };
      if (payload.message) message = payload.message;
      fields = payload.fields ?? {};
      code = payload.code ?? "";
    } catch {
      // No JSON body: the status code stays.
    }
    if (response.status === 422) message = t("settings:api.checkFields");
    if (response.status === 403) message = t("settings:api.adminOnly");
    throw new SettingsApiError(response.status, message, fields, code);
  }
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
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

  /** Forget what is saved and go back to the environment variables. */
  reset: (section: "ldap" | "mail" | "oauth") =>
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

/** An action of the catalog and the kind of resource it acts on (`null`:
 * the installation as a whole). */
export interface IamAction {
  name: string;
  resource: string | null;
}

export interface IamCatalog {
  actions: IamAction[];
  resource_kinds: string[];
}

export interface IamStatement {
  effect: "Allow" | "Deny";
  actions: string[];
  resources: string[];
}

export interface IamPolicy {
  name: string;
  description: string;
  statements: IamStatement[];
  /** Shipped with the application: attached, never changed. */
  managed: boolean;
  /** How many accounts, groups and service accounts have it. */
  attached: number;
}

export interface IamGroupMember {
  admin_id: string;
  email: string;
  display_name: string | null;
  /** `manual` (added here) or `directory` (by a directory group). */
  source: "manual" | "directory";
}

export interface IamGroup {
  id: string;
  name: string;
  description: string;
  directory_groups: string[];
  members: IamGroupMember[];
  policies: string[];
}

export interface IamGroupChange {
  name: string;
  description: string;
  directory_groups: string[];
  /** The people added by hand (account ids). */
  members: string[];
  policies: string[];
}

export interface UserAccess {
  admin_id: string;
  policies: string[];
  groups: string[];
}

export interface ServiceAccount {
  id: string;
  owner_id: string;
  owner_email: string;
  name: string;
  access_key: string;
  policies: string[];
  created_at: string;
  last_used_at: string | null;
  expires_at: string | null;
}

/** IAM (ADR 0016): policies, groups, what each account has, service accounts. */
export const iamApi = {
  catalog: () => request<IamCatalog>("GET", "/iam/catalog"),
  policies: () => request<IamPolicy[]>("GET", "/iam/policies"),
  putPolicy: (name: string, p: { description: string; statements: IamStatement[] }) =>
    request<void>("PUT", `/iam/policies/${encodeURIComponent(name)}`, p),
  deletePolicy: (name: string) =>
    request<void>("DELETE", `/iam/policies/${encodeURIComponent(name)}`),
  groups: () => request<IamGroup[]>("GET", "/iam/groups"),
  createGroup: (g: IamGroupChange) => request<{ id: string }>("POST", "/iam/groups", g),
  updateGroup: (id: string, g: IamGroupChange) =>
    request<void>("PUT", `/iam/groups/${encodeURIComponent(id)}`, g),
  deleteGroup: (id: string) => request<void>("DELETE", `/iam/groups/${encodeURIComponent(id)}`),
  users: () => request<UserAccess[]>("GET", "/iam/users"),
  setUserPolicies: (id: string, policies: string[]) =>
    request<void>("PUT", `/iam/users/${encodeURIComponent(id)}/policies`, { policies }),
  serviceAccounts: () => request<ServiceAccount[]>("GET", "/iam/service-accounts"),
  createServiceAccount: (sa: { name: string; policies: string[]; expires_at: string | null }) =>
    request<{ id: string; access_key: string; token: string }>("POST", "/iam/service-accounts", sa),
  deleteServiceAccount: (id: string) =>
    request<void>("DELETE", `/iam/service-accounts/${encodeURIComponent(id)}`),
};

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
