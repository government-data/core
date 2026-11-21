/**
 * The requests of the settings pages to the API: the CSRF header on every
 * change, the session's end, and the API's errors with their fields.
 */
import { t } from "./i18n";
import { sessionExpired } from "@knime-orchestador/feature-kit";

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

export function csrfHeader(): Record<string, string> {
  const token = document.cookie
    .split("; ")
    .find((row) => row.startsWith("csrf="))
    ?.slice("csrf=".length);
  return token ? { "x-csrf-token": token } : {};
}

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
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
