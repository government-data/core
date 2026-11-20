import { sessionExpired } from "@knime-orchestador/feature-kit";
import { locale } from "@knime-orchestador/i18n";
import { tr } from "./i18n";
/**
 * The platform's data layer: types of what `/api/*` returns and a thin
 * `fetch` that sends the session cookie and, on mutations, the CSRF header
 * the backend requires.
 */

export interface Me {
  admin_id: string;
  is_admin: boolean;
  /** The application's version. */
  version?: string;
  edition?: "onpremise" | "online";
  /** The service account the request comes with, if any. */
  service_account?: string | null;
  /** The actions it may do on some resource (ADR 0016); `admin`, everything. */
  actions?: string[];
  /** The contract about to end, ended or read-only (nothing if it is current). */
  license_notice?: ContractStatus | null;
  /** Maintenance (everyone), or a new version, one rolled back or a critical
   * state (administrators). */
  update_notice?: UpdateNotice | null;
}

/** The updates notice above every view. */
export type UpdateNotice =
  | { kind: "maintenance"; reason: string }
  | { kind: "critical" }
  | { kind: "rolled_back"; version: string; from: string }
  | { kind: "available"; version: string; mode: UpdateMode; failed: boolean };

export type UpdateMode = "manual" | "approval" | "automatic";

export interface UpdatePolicy {
  mode: UpdateMode;
  window: {
    /** 1 = Monday … 7 = Sunday. */
    days: number[];
    start: string;
    end: string;
    timezone: string;
  };
  max_drain_secs: number;
  skip_if_scheduled_within_secs: number;
  paused: boolean;
}

/** Where the server's update daemon is at. */
export type UpdatePhase =
  | "idle"
  | "prepared"
  | "draining"
  | "backing_up"
  | "applying"
  | "verifying"
  | "rolling_back"
  | "restoring"
  | "critical";

export type UpdateOutcomeKind =
  "updated" | "rolled_back" | "restored" | "critical" | "postponed" | "failed";

export interface UpdatesView {
  /** Whether the server's update service ever reported anything. */
  daemon: boolean;
  reported_at: string | null;
  /** It has not reported anything for more than 15 min. */
  stale: boolean;
  report: {
    phase: UpdatePhase | "";
    installed: string;
    available: { version: string; notes: string } | null;
    target: { version: string } | null;
    failed: string[];
    last: {
      kind: UpdateOutcomeKind;
      version: string;
      from: string;
      detail: string;
      at: string;
    } | null;
    handled_request: string | null;
    warning: string | null;
    /** Since when it is in the current phase (newer daemons). */
    phase_since?: string | null;
    /** What happens inside the phase ("2 flows running"). */
    progress?: string | null;
  };
  policy: UpdatePolicy;
  policy_updated_at: string | null;
  policy_updated_by: string | null;
  pending_request: { id: string; at: string; by: string | null } | null;
  maintenance: { reason: string; started_at: string; until: string } | null;
  history: {
    kind: UpdateOutcomeKind;
    version: string;
    from_version: string;
    detail: string;
    at: string;
  }[];
}

export interface LicenseLimits {
  servers: number | null;
  workflows: number | null;
  users: number | null;
  executions_month: number | null;
  deployments_month: number | null;
}

export type LicenseUsage = Record<keyof LicenseLimits, number>;

/**
 * Where the contract is at. Past the grace period, or suspended, the
 * installation goes read-only: everything can be looked at and what is
 * scheduled keeps running.
 */
export type ContractStatus =
  | { state: "current" }
  | { state: "ending_soon"; ends_at: string; days_left: number }
  | { state: "grace"; ended_at: string; read_only_from: string }
  | { state: "read_only"; reason: string };

/** A confirmed payment of the license (the seller records it). */
export interface BillingPayment {
  /** The electronic invoice's number. */
  invoice: string;
  /** The day of the payment (`2026-10-15`). */
  paid_on: string;
  amount_cop: number;
  period: string;
  /** The contract period it covers. */
  from: string;
  to: string;
}

/** The license's billing, as the cloud keeps it. */
export interface LicenseBilling {
  billing: {
    period: string | null;
    packages: Record<string, number>;
    /** What renewing would cost: the last payment, for the same period. */
    renewal_cop: number | null;
    payments: BillingPayment[];
  } | null;
  fetched_at: string | null;
  /** If the cloud did not answer: the last it gave is shown. */
  error: string | null;
}

/** The installation's license and how much of it is used. */
export interface LicenseInfo {
  license: {
    status:
      { state: "active" } | { state: "grace"; until: string } | { state: "basic"; reason: string };
    entitlement: {
      customer: string;
      plan: string;
      features: string[];
      limits: LicenseLimits;
      installation: string;
      issued_at: string;
      expires_at: string;
      /** Until when the contract is paid (without it: it does not end). */
      contract_ends_at?: string;
      suspended?: string;
    } | null;
    limits: LicenseLimits;
    contract: ContractStatus | null;
    installation: string;
    key_hint: string | null;
    fetched_at: string | null;
    last_attempt_at: string | null;
    last_error: string | null;
    dev_key: boolean;
  };
  usage: LicenseUsage;
}

/** An API error, with the readable message the backend returns. */
export class PlatformApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
    /** The API's stable code (`code_review`, `conflict`...), if it came. */
    readonly code?: string,
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
    throw new PlatformApiError(401, tr("platform:api.sessionExpired"));
  }
  if (!response.ok) throw await apiError(response);
  // No body: a 204, or a 202 of a request handled later.
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** The error of an unsuccessful response, with the API's message and code. */
async function apiError(response: Response): Promise<PlatformApiError> {
  let message = `Error ${response.status}`;
  let code: string | undefined;
  try {
    const payload = (await response.json()) as { message?: string; code?: string };
    if (payload.message) message = payload.message;
    code = payload.code;
  } catch {
    if (response.status === 413) message = tr("platform:api.chunkTooLarge");
  }
  return new PlatformApiError(response.status, message, code);
}

const get = <T>(path: string) => request<T>("GET", path);
const post = <T>(path: string, body?: unknown) => request<T>("POST", path, body ?? {});
const put = <T>(path: string, body: unknown) => request<T>("PUT", path, body);

export const platformApi = {
  me: () => get<Me>("/access/me"),

  /** The license (`null` if the installation does not use one). */
  license: () => get<LicenseInfo | null>("/license"),
  setLicenseKey: (key: string) => request<void>("PUT", "/license/key", { key }),
  refreshLicense: () => post<void>("/license/refresh"),
  licenseBilling: () => get<LicenseBilling>("/license/billing"),
  /** The updates (`null` if the installation is not updated from here). */
  updates: () => get<UpdatesView | null>("/updates"),
  setUpdatePolicy: (policy: UpdatePolicy) => put<void>("/updates/policy", policy),
  /** "Update now": the server's service handles it in less than a minute. */
  requestUpdate: () => post<{ id: string; at: string }>("/updates/request"),
};

const dateFormat = new Intl.DateTimeFormat(locale(), {
  dateStyle: "short",
  timeStyle: "medium",
});

export function formatDate(iso: string | null | undefined): string {
  return iso ? dateFormat.format(new Date(iso)) : "—";
}

const dayFormat = new Intl.DateTimeFormat(locale(), { dateStyle: "long" });

/** Only the day, in words: «4 de diciembre de 2026». */
export function formatDay(iso: string | null | undefined): string {
  return iso ? dayFormat.format(new Date(iso)) : "—";
}

export function formatDuration(from: string | null, to: string | null): string {
  if (!from) return "—";
  const ms = (to ? new Date(to) : new Date()).getTime() - new Date(from).getTime();
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  if (s < 3600) return tr("platform:api.floorMinSS", { floor: Math.floor(s / 60), s: s % 60 });
  return tr("platform:api.floorHFloor3Min", {
    floor: Math.floor(s / 3600),
    floor3: Math.floor((s % 3600) / 60),
  });
}

/** Whether the actor may do `action` on some resource (`admin`, all). */
export function may(me: Me | undefined, action: string): boolean {
  return !!me && (me.is_admin || (me.actions ?? []).includes(action));
}
