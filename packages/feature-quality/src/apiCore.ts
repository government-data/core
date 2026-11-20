/**
 * The typed client over the API's OpenAPI contract that every part of the
 * quality feature's data layer calls through: a refused call as an error,
 * and the CSRF header the mutations need.
 */

import { createApiClient } from "@knime-orchestador/api-client";
import { sessionExpired } from "@knime-orchestador/feature-kit";

/** A refused call: its status, and the body the API sent with it. */
export class QualityError extends Error {
  constructor(
    readonly status: number,
    readonly body: unknown,
  ) {
    super(`quality API ${status}`);
  }
}

export const api = createApiClient();

/** The data of a call, or a {@link QualityError}; a lost session signs in again. */
export async function done<T>(
  call: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, error, response } = await call;
  if (response.status === 401) sessionExpired();
  if (!response.ok) throw new QualityError(response.status, error);
  return data as T;
}

/** The `X-CSRF-Token` header, from the client-readable `csrf` cookie. */
export function csrf(): Record<string, string> {
  const token = document.cookie
    .split("; ")
    .find((c) => c.startsWith("csrf="))
    ?.slice("csrf=".length);
  return token ? { "x-csrf-token": decodeURIComponent(token) } : {};
}
