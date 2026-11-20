import type { UpdatePhase, UpdatesView } from "./api";
import { tr, translated } from "./i18n";

/**
 * The bar of an update in progress: how far it is (by phases, with their
 * weight), what it is doing and since when. Pure, to test it without the page.
 */
export interface UpdateProgress {
  /** 0–100: where the current phase's stretch begins. */
  percent: number;
  /** Where the stretch ends (the animated stripe runs along it). */
  until: number;
  label: string;
  /** What the daemon reports inside the phase, and how long it has taken. */
  detail: string;
  tone: "run" | "warn" | "ok" | "bad";
}

/** Each phase's stretch: waiting for the flows can take a while, the rest not. */
const SPANS: Partial<Record<UpdatePhase, [number, number, string]>> = translated(() => ({
  draining: [10, 35, tr("platform:updateProgress.draining")],
  backing_up: [35, 55, tr("platform:updateProgress.backingUpDatabase")],
  applying: [55, 75, tr("platform:updateProgress.installingNewVersion")],
  verifying: [75, 95, tr("platform:updateProgress.checkingNewVersionResponds")],
  rolling_back: [60, 90, tr("platform:updateProgress.rollingBack")],
  restoring: [60, 90, tr("platform:updateProgress.restoringDatabaseBackup")],
}));

/** «3 min», «45 s», «1 h 5 min». */
export function elapsed(since: string | null | undefined, now: Date): string {
  if (!since) return "";
  const secs = Math.max(0, Math.round((now.getTime() - new Date(since).getTime()) / 1000));
  if (secs < 60) return `${secs} s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins} min`;
  const rest = mins % 60;
  return rest
    ? tr("platform:updateProgress.floorHRestMin", { floor: Math.floor(mins / 60), rest: rest })
    : `${Math.floor(mins / 60)} h`;
}

/**
 * The bar for what is seen now, or `null` if nothing is in progress.
 * `restarting`: the API does not answer halfway through the installation (it
 * is being replaced), which is expected and not an error.
 */
export function updateProgress(
  v: UpdatesView,
  now: Date,
  restarting = false,
): UpdateProgress | null {
  const phase = v.report.phase as UpdatePhase;
  if (restarting) {
    return {
      percent: 55,
      until: 90,
      label: tr("platform:updateProgress.restarting"),
      detail: tr("platform:updateProgress.backSoon"),
      tone: "run",
    };
  }
  const span = SPANS[phase];
  if (span) {
    const [percent, until, label] = span;
    const parts = [v.report.progress, elapsed(v.report.phase_since, now)].filter(Boolean);
    return {
      percent,
      until,
      label,
      detail: parts.join(" · "),
      tone: phase === "rolling_back" || phase === "restoring" ? "warn" : "run",
    };
  }
  if (v.pending_request) {
    return {
      percent: 3,
      until: 10,
      label: tr("platform:updateProgress.requestSent"),
      detail: tr("platform:updateProgress.pickedUpSoon"),
      tone: "run",
    };
  }
  if (phase === "critical") {
    return {
      percent: 100,
      until: 100,
      label: tr("platform:updateProgress.failedCritical"),
      detail: tr("platform:updateProgress.checkServer"),
      tone: "bad",
    };
  }
  return null;
}
