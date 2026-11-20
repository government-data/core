import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { A } from "@solidjs/router";
import { Status } from "@knime-orchestador/feature-kit/ui";
import { formatNumber } from "@knime-orchestador/i18n";
import { cutsApi, may } from "./api";
import { Branch } from "./Branch";
import { allSchedules, scheduled, who } from "./explorerCache";
import { t } from "./i18n";
import { LoadFailed, notify, warn } from "./Notify";
import { AddSchedule, ScheduleItem } from "./ScheduleLeaves";
import { cutState } from "./schedules";

/**
 * _Run now_ at the head of a dataset's cuts, for whoever may run its
 * pipeline: today's cut, blocked or replaced as asked; what came of it
 * said in a toast.
 */
const RunNow: Component<{ dataset: string; onRun: () => void }> = (props) => {
  const [rerun, setRerun] = createSignal<"block" | "replace">("block");
  const run = async () => {
    try {
      const out = await cutsApi.run(props.dataset, rerun());
      notify(t(`quality:cuts.run.${out.outcome}`, { cut: out.cut }));
    } catch (err) {
      const body = (err as { body?: { outcome?: string; cut?: string; error?: string } }).body;
      if (body?.outcome === "blocked") warn(t("quality:cuts.run.blocked", { cut: body.cut ?? "" }));
      else if (body?.error === "no_source") warn(t("quality:cuts.run.noSource"));
      else notify(t("quality:common.saveError"), true);
    }
    props.onRun();
  };
  return (
    <li role="treeitem" class="qd-tree-leaf qd-run">
      <select
        aria-label={t("quality:cuts.rerun")}
        value={rerun()}
        onChange={(e) => setRerun(e.currentTarget.value as "block" | "replace")}
      >
        <option value="block">{t("quality:cuts.rerunBlock")}</option>
        <option value="replace">{t("quality:cuts.rerunReplace")}</option>
      </select>{" "}
      <button type="button" class="fk-btn fk-btn--sm fk-btn--primary" onClick={() => void run()}>
        {t("quality:cuts.runNow")}
      </button>
    </li>
  );
};

/** A dataset's cuts, newest first: _Run now_, then each date, its state and its records. */
const Cuts: Component<{ dataset: string }> = (props) => {
  const [cuts, { refetch }] = createResource(() => props.dataset, cutsApi.list);
  const [me] = createResource(who);
  return (
    <>
      <Show when={may(me(), "quality:RunPipeline")}>
        <RunNow dataset={props.dataset} onRun={() => void refetch()} />
      </Show>
      <LoadFailed when={cuts.error} />
      <For each={cuts()} fallback={<li class="qd-hint">{t("quality:cuts.empty")}</li>}>
        {(cut) => (
          <li role="treeitem" class="qd-tree-leaf">
            <span class="fk-num">{cut.cut}</span> <Status {...cutState(cut.state)} />{" "}
            <Show when={cut.records != null}>
              <small class="qd-hint">
                {t("quality:explorer.records", { n: formatNumber(cut.records ?? 0) })}
              </small>
            </Show>
            <Show when={cut.failed_at}>
              {(at) => (
                <small class="qd-error" title={cut.error ?? undefined}>
                  {t(`quality:cuts.failedAt.${at()}`)}
                  {cut.error ? `: ${cut.error}` : ""}
                </small>
              )}
            </Show>
          </li>
        )}
      </For>
    </>
  );
};

/**
 * A dataset's cuts and its schedules with _+_ to add one (decision 0049,
 * point 2), only for whoever may view its pipeline; last, its dashboard
 * (decision 0031), for whoever may view results.
 */
export const DatasetPlan: Component<{ dataset: string; pipelines: boolean }> = (props) => {
  const [schedules] = createResource(
    () => (props.pipelines ? scheduled() : undefined),
    allSchedules,
  );
  const mine = () => (schedules() ?? []).filter((s) => s.dataset === props.dataset);
  const [me] = createResource(who);
  return (
    <>
      <Show when={props.pipelines}>
        <Branch class="qd-tree-group" label={<em>{t("quality:explorer.cuts")}</em>}>
          {() => <Cuts dataset={props.dataset} />}
        </Branch>
      </Show>
      <Show when={props.pipelines}>
        <Branch
          class="qd-tree-group"
          label={
            <span>
              <em>{t("quality:explorer.schedules")}</em> <AddSchedule dataset={props.dataset} />
            </span>
          }
        >
          {() => (
            <For each={mine()} fallback={<li class="qd-hint">{t("quality:schedules.empty")}</li>}>
              {(s) => <ScheduleItem schedule={s} />}
            </For>
          )}
        </Branch>
      </Show>
      <Show when={may(me(), "quality:ViewResults")}>
        <li role="treeitem" class="qd-tree-leaf">
          <A class="qd-link" href={`/quality/dashboard/${encodeURIComponent(props.dataset)}`}>
            <em>{t("quality:explorer.dashboard")}</em>
          </A>
        </li>
      </Show>
    </>
  );
};
