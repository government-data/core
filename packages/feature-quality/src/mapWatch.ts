import { createSignal } from "solid-js";
import { toast } from "@knime-orchestador/feature-kit/ui";
import { formatNumber } from "@knime-orchestador/i18n";
import { sourcesApi } from "./api";
import { mappingsChanged } from "./explorerCache";
import { t } from "./i18n";
import { stage, type MapRunning, type MapStage } from "./mapStage";

/** How often a map in course is asked about. */
const EVERY_MS = 5000;

/** A source's table, as the explorer places it: `source/schema.table`. */
export const tableKey = (source: string, table: string) => `${source}/${table}`;

/** A table being loaded: the dataset it becomes, what is being done, and since when. */
export interface Loading {
  dataset: string;
  at: MapRunning;
  since: Date;
}

/** The tables being loaded, until each is ready or failed. */
const [loading, setLoading] = createSignal<ReadonlyMap<string, Loading>>(new Map());
export { loading };

const mark = (key: string, dataset: string, at: MapRunning) =>
  setLoading((all) => {
    const was = all.get(key);
    if (was?.at === at) return all;
    return new Map(all).set(key, { dataset, at, since: was?.since ?? new Date() });
  });

const settle = (key: string) =>
  setLoading((all) => {
    const left = new Map(all);
    left.delete(key);
    return left;
  });

/**
 * Follows a map (decision 0053) while the app is open: its table shows as
 * loading in the explorer, the dataset shows once it is on `main`, and when
 * its first cut is evaluated, or anything failed, a toast says so and stays
 * until closed.
 */
export function watchMap(job: string, dataset: string, source: string, table: string) {
  const key = tableKey(source, table);
  mark(key, dataset, "queued");
  let mapped = false;
  const tick = async () => {
    let now: MapStage;
    try {
      now = stage(await sourcesApi.mapProgress(job));
    } catch {
      setTimeout(() => void tick(), EVERY_MS * 3);
      return;
    }
    if (now.at !== "queued" && now.at !== "mapping" && !mapped) {
      mapped = true;
      mappingsChanged();
    }
    if (now.at === "ready" || now.at === "failed") settle(key);
    if (now.at === "ready") {
      const records = formatNumber(now.records);
      toast("success", t("quality:sources.map.ready", { dataset, records }), undefined, null);
    } else if (now.at === "failed") {
      const title = t(`quality:sources.map.failed.${now.step}`, { dataset });
      toast("error", title, now.error || undefined, null);
    } else {
      mark(key, dataset, now.at);
      setTimeout(() => void tick(), EVERY_MS);
    }
  };
  setTimeout(() => void tick(), EVERY_MS);
}
