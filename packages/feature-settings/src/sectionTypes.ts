/**
 * What each section of the system settings holds (`GET /settings`).
 */
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
  /** Set by the platform (management panel): the server is not shown or changed here. */
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

/** The platform's own settings (its time zone, an IANA name). */
export interface PlatformSettings {
  timezone: string;
}

export interface AllSettings {
  ldap: Section<LdapSettings>;
  mail: Section<MailSettings>;
  oauth: Section<OAuthSettings>;
  platform: Section<PlatformSettings>;
  /** The redirect URL with `{id}` in place of the provider's id. */
  oauth_redirect_template: string;
  /** Minutes the recovery link is valid when the mail settings do not set others. */
  reset_link_default_minutes: number;
  /** Where the key that encrypts the secrets comes from. */
  encryption_key: "environment" | "file" | "generated";
  /** The platform's sender for whoever does not configure their own mail
   * (the management panel's); `null` if there is none. */
  platform_mail_from?: string | null;
}
