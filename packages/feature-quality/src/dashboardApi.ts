/**
 * A dataset's dashboard (decision 0031) as the API serves it: its charts
 * and one's rows for a cut, and its remediation queues' SLAs (decision
 * 0059), read by whoever sees its results and replaced whole by whoever
 * may edit its pipeline.
 */

import { type components } from "@knime-orchestador/api-client";
import { api, csrf, done } from "./apiCore";

export type SlasOut = components["schemas"]["SlasOut"];
export type SlasBody = components["schemas"]["SlasBody"];

/** A dataset's dashboard (decision 0031): its charts, and one's rows for a cut. */
export const chartsApi = {
  list: (dataset: string) =>
    done(api.GET("/quality/datasets/{dataset}/charts", { params: { path: { dataset } } })),
  rows: (dataset: string, chart: string, cut?: string) =>
    done(
      api.GET("/quality/datasets/{dataset}/charts/{chart}", {
        params: { path: { dataset, chart }, query: { cut } },
      }),
    ),
};

export const slasApi = {
  read: (dataset: string) =>
    done(api.GET("/quality/datasets/{dataset}/queues/slas", { params: { path: { dataset } } })),
  replace: (dataset: string, body: SlasBody) =>
    done(
      api.PUT("/quality/datasets/{dataset}/queues/slas", {
        params: { path: { dataset } },
        body,
        headers: csrf(),
      }),
    ),
};
