import type { MapProgress } from "./api";

/** A map in course: waiting for a worker, sampling, then its cut extracting and evaluating. */
export type MapRunning = "queued" | "mapping" | "extracting" | "evaluating";

/** Where a map is: in course, or over. */
export type MapStage =
  | { at: MapRunning }
  | { at: "ready"; records: number }
  | { at: "failed"; step: "map" | "extract" | "evaluate"; error: string };

/** The stage of a map from what `GET /quality/maps/{job}` says. */
export function stage(p: MapProgress): MapStage {
  if (p.state === "failed") return { at: "failed", step: "map", error: p.error ?? "" };
  if (p.state === "queued") return { at: "queued" };
  if (p.state !== "done" || !p.cut) return { at: "mapping" };
  if (p.cut.state === "done") return { at: "ready", records: p.cut.records ?? 0 };
  if (p.cut.state === "failed") {
    const step = p.cut.failed_at === "extract" ? "extract" : "evaluate";
    return { at: "failed", step, error: p.cut.error ?? "" };
  }
  return { at: p.cut.state === "evaluating" ? "evaluating" : "extracting" };
}
