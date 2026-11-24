/**
 * The quality feature's data layer over the client of `apiCore.ts`
 * (decision 0036's sources, 0038's maps, the drafts and the cuts), and who
 * the person is and may do (IAM, ADR 0016); the dashboard's calls are in
 * `dashboardApi.ts`.
 */

import { type components } from "@knime-orchestador/api-client";
import { sessionExpired } from "@knime-orchestador/feature-kit";
import { api, csrf, done, noCatalog, QualityError } from "./apiCore";

export { noCatalog, QualityError };

export type Driver = components["schemas"]["DriverOut"];
export type DriverField = components["schemas"]["DriverFieldOut"];
export type SourceView = components["schemas"]["SourceView"];
export type SourceBody = components["schemas"]["SourceBody"];
export type Refusal = components["schemas"]["Refusal"];
export type MapOut = components["schemas"]["MapOut"];
export type ListingView = components["schemas"]["ListingView"];
export type SourceTable = components["schemas"]["SourceTable"];
export type SourceColumn = components["schemas"]["SourceColumn"];

export const sourcesApi = {
  drivers: () => done(api.GET("/quality/drivers")),
  list: () => done(api.GET("/quality/sources")),
  put: (name: string, body: SourceBody) =>
    done(api.PUT("/quality/sources/{name}", { params: { path: { name } }, body, headers: csrf() })),
  remove: (name: string) =>
    done(api.DELETE("/quality/sources/{name}", { params: { path: { name } }, headers: csrf() })),
  tables: (name: string) =>
    done(api.GET("/quality/sources/{name}/tables", { params: { path: { name } } })),
  listTables: (name: string) =>
    done(
      api.POST("/quality/sources/{name}/tables", { params: { path: { name } }, headers: csrf() }),
    ),
  map: (name: string, table: string, dataset: string) =>
    done(
      api.POST("/quality/sources/{name}/maps", {
        params: { path: { name } },
        body: { table, dataset },
        headers: csrf(),
      }),
    ),
  mapProgress: (job: string) => done(api.GET("/quality/maps/{job}", { params: { path: { job } } })),
};

/** A source table's records (decision 0049): a page asked, then read when its job is done. */
export const recordsApi = {
  ask: (name: string, body: RecordsAsk) =>
    done(
      api.POST("/quality/sources/{name}/records", {
        params: { path: { name } },
        body,
        headers: csrf(),
      }),
    ),
  read: (name: string, job: string) =>
    done(api.GET("/quality/sources/{name}/records/{job}", { params: { path: { name, job } } })),
};

export type RecordsAsk = components["schemas"]["RecordsAsk"];
export type RecordsOut = components["schemas"]["RecordsOut"];
export type RecordsPage = components["schemas"]["RecordsPage"];

/** The lake listed as a source (decision 0043): its last listing, and a new one. */
export const lakeApi = {
  tables: () => done(api.GET("/quality/lake/tables")),
  listTables: () => done(api.POST("/quality/lake/tables", { headers: csrf() })),
};

/** Who I am and what I may do: the actions on some resource. */
export interface Me {
  is_admin: boolean;
  actions: string[];
  /** The platform's time zone (decision 0079), UTC when never set. */
  platform_timezone?: string;
}

/** The person's actions (`/access/me`, as Settings reads them). */
export async function me(): Promise<Me> {
  const res = await fetch("/api/access/me", { credentials: "include" });
  if (res.status === 401) sessionExpired();
  if (!res.ok) throw new QualityError(res.status, undefined);
  return (await res.json()) as Me;
}

/** Whether `who` may do `action` on some resource; `admin`, everything. */
export function may(who: Me | undefined, action: string): boolean {
  return !!who && (who.is_admin || who.actions.includes(action));
}

export type DatasetOut = components["schemas"]["DatasetOut"];
export type CutOut = components["schemas"]["CutOut"];
export type RunOut = components["schemas"]["RunOut"];
export type ScheduleOut = components["schemas"]["ScheduleOut"];
export type MapProgress = components["schemas"]["MapProgress"];
export type ConsoleOut = components["schemas"]["ConsoleOut"];

/** Deleting a dataset or a lake table, its name typed again (decision 0055). */
export const deleteApi = {
  dataset: (dataset: string, confirm: string) =>
    done(
      api.DELETE("/quality/datasets/{dataset}", {
        params: { path: { dataset } },
        body: { confirm },
        headers: csrf(),
      }),
    ),
  lakeNamespace: (namespace: string, confirm: string) =>
    done(
      api.DELETE("/quality/lake/namespaces/{namespace}", {
        params: { path: { namespace } },
        body: { confirm },
        headers: csrf(),
      }),
    ),
  lakeTable: (namespace: string, table: string, confirm: string) =>
    done(
      api.DELETE("/quality/lake/tables/{namespace}/{table}", {
        params: { path: { namespace, table } },
        body: { confirm },
        headers: csrf(),
      }),
    ),
};

/** The console over the tenant's lake (decision 0054). */
export const consoleApi = {
  run: (sql: string) => done(api.POST("/quality/console", { body: { sql }, headers: csrf() })),
  read: (job: string) => done(api.GET("/quality/console/{job}", { params: { path: { job } } })),
};
export type ScheduleBody = components["schemas"]["ScheduleBody"];

export const cutsApi = {
  datasets: () => done(api.GET("/quality/datasets")),
  list: (dataset: string) =>
    done(api.GET("/quality/datasets/{dataset}/cuts", { params: { path: { dataset } } })),
  run: (dataset: string, rerun: "block" | "replace") =>
    done(
      api.POST("/quality/datasets/{dataset}/cuts", {
        params: { path: { dataset } },
        body: { rerun },
        headers: csrf(),
      }),
    ),
};

export const schedulesApi = {
  list: () => done(api.GET("/quality/schedules")),
  create: (body: ScheduleBody) => done(api.POST("/quality/schedules", { body, headers: csrf() })),
  update: (id: string, body: ScheduleBody) =>
    done(api.PUT("/quality/schedules/{id}", { params: { path: { id } }, body, headers: csrf() })),
  remove: (id: string) =>
    done(api.DELETE("/quality/schedules/{id}", { params: { path: { id } }, headers: csrf() })),
};

export type DraftInfo = components["schemas"]["DraftInfo"];
export type DraftOut = components["schemas"]["DraftOut"];

export type DraftHistory = components["schemas"]["DraftHistory"];
export type DraftCommit = components["schemas"]["DraftCommit"];
export type FileChange = components["schemas"]["FileChange"];
export type MainChange = components["schemas"]["MainChange"];

export const draftsApi = {
  list: () => done(api.GET("/quality/drafts")),
  /** A draft's commits, newest first, and where it left `main`. */
  history: (dataset: string, id: string) =>
    done(api.GET("/quality/drafts/{dataset}/{id}/history", { params: { path: { dataset, id } } })),
  /** What the whole draft changed, or one of its commits. */
  changes: (dataset: string, id: string, commit?: string) =>
    done(
      api.GET("/quality/drafts/{dataset}/{id}/changes", {
        params: { path: { dataset, id }, query: commit ? { commit } : {} },
      }),
    ),
  read: (dataset: string, id: string) =>
    done(api.GET("/quality/drafts/{dataset}/{id}", { params: { path: { dataset, id } } })),
  /** A column's changes on `main`: its rules file's, or the mapping's without one (0050). */
  fileHistory: (dataset: string, file?: string) =>
    done(
      api.GET("/quality/datasets/{dataset}/history", {
        params: { path: { dataset }, query: file ? { file } : {} },
      }),
    ),
  approve: (dataset: string, id: string) =>
    done(
      api.POST("/quality/drafts/{dataset}/{id}/approve", {
        params: { path: { dataset, id } },
        headers: csrf(),
      }),
    ),
  discard: (dataset: string, id: string) =>
    done(
      api.DELETE("/quality/drafts/{dataset}/{id}", {
        params: { path: { dataset, id } },
        headers: csrf(),
      }),
    ),
};

export type RulesOut = components["schemas"]["RulesOut"];
export type RuleView = components["schemas"]["RuleView"];
export type FindingOut = components["schemas"]["FindingOut"];
export type CheckOut = components["schemas"]["CheckOut"];
export type TestOut = components["schemas"]["TestOut"];
export type RuleTested = components["schemas"]["RuleTested"];
export type TestedRecord = components["schemas"]["TestedRecord"];

export const rulesApi = {
  /** The datasets whose rules the person may see. */
  datasets: () => done(api.GET("/quality/datasets", { params: { query: { for: "rules" } } })),
  read: (dataset: string) =>
    done(api.GET("/quality/datasets/{dataset}/rules", { params: { path: { dataset } } })),
  /**
   * `file` named, also the rules not in their field's file (decision 0044);
   * on `draft`, with its global rules (0045).
   */
  check: (dataset: string, yaml: string, file?: string, draft?: string) =>
    done(
      api.POST("/quality/rules/check", { body: { dataset, yaml, file, draft }, headers: csrf() }),
    ),
  /** A rules file tidied (decision 0041): it reads the same. */
  tidy: (yaml: string) =>
    done(api.POST("/quality/rules/tidy", { body: { yaml }, headers: csrf() })),
  /** Tests a rules file as typed on the dataset's latest cut (decision 0043); `rule` only it. */
  test: (dataset: string, yaml: string, rule?: string, columns: string[] = []) =>
    done(
      api.POST("/quality/rules/test", { body: { dataset, yaml, rule, columns }, headers: csrf() }),
    ),
  /** A test's state and, when done, its counts and records. */
  readTest: (job: string) =>
    done(api.GET("/quality/rules/tests/{job}", { params: { path: { job } } })),
  /** The JSON Schema of a rules file, for completion. */
  schema: () => done(api.GET("/quality/rules/schema")) as Promise<Record<string, unknown>>,
};

export type SavedOut = components["schemas"]["SavedOut"];

export const editsApi = {
  /**
   * Splits a dataset's rules by field on a new draft (decisions 0044, 0045);
   * nothing when they already are.
   */
  split: (dataset: string) =>
    done(
      api.POST("/quality/datasets/{dataset}/split", {
        params: { path: { dataset } },
        headers: csrf(),
      }),
    ),
  /**
   * Maps a table's columns no field reads as fields of the dataset, on a
   * new draft (decision 0043); with `table`, a table no dataset reads
   * starts the dataset.
   */
  addFields: (dataset: string, body: components["schemas"]["FieldsBody"]) =>
    done(
      api.POST("/quality/datasets/{dataset}/fields", {
        params: { path: { dataset } },
        body,
        headers: csrf(),
      }),
    ),
  /**
   * Deploys a column's file to `main` (decision 0051), over `expected`, the
   * file as it was opened (`null` if `main` had none).
   */
  deploy: (dataset: string, file: string, yaml: string, expected: string | null) =>
    done(
      api.PUT("/quality/datasets/{dataset}/files/{file}", {
        params: { path: { dataset, file } },
        body: { yaml, expected },
        headers: csrf(),
      }),
    ),
  /** Starts a draft from `main` with `file` edited. */
  start: (dataset: string, file: string, yaml: string) =>
    done(
      api.POST("/quality/datasets/{dataset}/drafts", {
        params: { path: { dataset } },
        body: { file, yaml },
        headers: csrf(),
      }),
    ),
  /** Saves `file` on draft `id`. */
  save: (dataset: string, id: string, file: string, yaml: string) =>
    done(
      api.PUT("/quality/drafts/{dataset}/{id}/files/{file}", {
        params: { path: { dataset, id, file } },
        body: { yaml },
        headers: csrf(),
      }),
    ),
};
