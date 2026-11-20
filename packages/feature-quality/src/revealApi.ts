/**
 * The reveals of decision 0069 (point 2): a source's page, a rule test's
 * records and a console result shown whole, each a `POST` beside its read
 * with the reason in its body and the CSRF header.
 */

import type { createApiClient } from "@knime-orchestador/api-client";
import { api, csrf, done } from "./apiCore";
import type { Reason } from "./reveal";

/** The three reveals over `client` (the app's, or a test's). */
export const revealApiWith = (client: ReturnType<typeof createApiClient>) => ({
  records: (name: string, job: string, body: Reason) =>
    done(
      client.POST("/quality/sources/{name}/records/{job}/reveal", {
        params: { path: { name, job } },
        body,
        headers: csrf(),
      }),
    ),
  test: (job: string, body: Reason) =>
    done(
      client.POST("/quality/rules/tests/{job}/reveal", {
        params: { path: { job } },
        body,
        headers: csrf(),
      }),
    ),
  console: (job: string, body: Reason) =>
    done(
      client.POST("/quality/console/{job}/reveal", {
        params: { path: { job } },
        body,
        headers: csrf(),
      }),
    ),
});

export const revealApi = revealApiWith(api);
