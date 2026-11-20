/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { A } from "@solidjs/router";
import {
  createEffect,
  createResource,
  createSignal,
  For,
  onCleanup,
  Show,
  type Component,
} from "solid-js";
import { ConfirmDialog } from "@knime-orchestador/feature-kit/ui";
import {
  formatDate,
  platformApi,
  type UpdateMode,
  type UpdateOutcomeKind,
  type UpdatePhase,
  type UpdatePolicy,
  type UpdatesView,
} from "./api";
import { attempt, ErrorBox, Page, refetchMe } from "./common";
import { updateProgress } from "./updateProgress";
import { tr, translated } from "./i18n";
import { Icon } from "./icons";
import { WEEKDAYS } from "./scheduleTrigger";

/** What the server's service does in each phase, in words. */
const PHASE_LABEL: Record<UpdatePhase, string> = translated(() => ({
  idle: tr("platform:updates.noPendingUpdates"),
  prepared: tr("platform:updates.ready"),
  draining: tr("platform:updates.draining"),
  backing_up: tr("platform:updates.backingUpDatabase"),
  applying: tr("platform:updates.installingNewVersion"),
  verifying: tr("platform:updates.checkingNewVersionResponds"),
  rolling_back: tr("platform:updates.newVersionDidNot"),
  restoring: tr("platform:updates.restoringDatabaseBackup"),
  critical: tr("platform:updates.failedCritical"),
}));

/** The phases of an update that goes well, in order. */
const STEPS: { phase: UpdatePhase; label: string }[] = translated(() => [
  { phase: "draining", label: tr("platform:updates.waitWorkflows") },
  { phase: "backing_up", label: tr("platform:updates.backUp") },
  { phase: "applying", label: tr("platform:updates.install") },
  { phase: "verifying", label: tr("platform:updates.check") },
]);

const IN_PROGRESS: UpdatePhase[] = [
  "draining",
  "backing_up",
  "applying",
  "verifying",
  "rolling_back",
  "restoring",
];

const OUTCOME_LABEL: Record<UpdateOutcomeKind, string> = translated(() => ({
  updated: tr("platform:updates.updated"),
  rolled_back: tr("platform:updates.rolledBack"),
  restored: tr("platform:updates.restored"),
  critical: tr("platform:updates.critical"),
  postponed: tr("platform:updates.postponed"),
  failed: tr("platform:updates.couldNotPrepare"),
}));

const OUTCOME_TONE: Record<UpdateOutcomeKind, "ok" | "warn" | "bad"> = {
  updated: "ok",
  rolled_back: "warn",
  restored: "warn",
  critical: "bad",
  postponed: "warn",
  failed: "warn",
};

const MODES: { mode: UpdateMode; label: string; hint: string }[] = translated(() => [
  {
    mode: "approval",
    label: tr("platform:updates.withApproval"),
    hint: tr("platform:updates.withApprovalHelp"),
  },
  {
    mode: "automatic",
    label: tr("platform:updates.automatic"),
    hint: tr("platform:updates.automaticHelp"),
  },
  {
    mode: "manual",
    label: tr("platform:updates.manual"),
    hint: tr("platform:updates.manualHelp"),
  },
]);

/** The days (abbreviated) in the active language, Monday to Sunday. */
const DAYS = WEEKDAYS.map((d) => d.short[0]!.toUpperCase() + d.short.slice(1));

const minutes = (secs: number) => Math.round(secs / 60);

/** Settings → Updates (administrators only, local installation). */
export const Updates: Component = () => {
  /**
   * Halfway through the installation the API is replaced and does not
   * answer: that is expected. Meanwhile the last state keeps showing, as
   * restarting.
   */
  const [restarting, setRestarting] = createSignal(false);
  let last: UpdatesView | null | undefined;
  const [view, { refetch }] = createResource(async () => {
    try {
      const v = await platformApi.updates();
      setRestarting(false);
      last = v;
      return v;
    } catch (e) {
      const midway =
        !!last &&
        (IN_PROGRESS.includes(last.report.phase as UpdatePhase) || !!last.pending_request);
      if (midway) {
        setRestarting(true);
        return last!;
      }
      throw e;
    }
  });
  /** The time, for how long each phase has taken (every second while running). */
  const [now, setNow] = createSignal(new Date());
  /** It finished well while being watched: the new UI is one reload away. */
  const [justUpdated, setJustUpdated] = createSignal<string>();
  const [error, setError] = createSignal<unknown>();
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [policy, setPolicy] = createSignal<UpdatePolicy>();

  // The policy being edited starts from the saved one (and returns to it on save).
  createEffect(() => {
    const v = view();
    if (v && !policy()) setPolicy(structuredClone(v.policy));
  });

  // While something is in progress (or requested), it is checked often.
  const active = () => {
    const v = view();
    return !!v && (IN_PROGRESS.includes(v.report.phase as UpdatePhase) || !!v.pending_request);
  };
  createEffect(() => {
    if (!active()) return;
    const timer = setInterval(() => setNow(new Date()), 1_000);
    onCleanup(() => clearInterval(timer));
  });
  createEffect(() => {
    const every = active() ? 2_000 : 60_000;
    const timer = setInterval(() => {
      if (!document.hidden) void refetch();
    }, every);
    onCleanup(() => clearInterval(timer));
  });
  // When it finishes, the notice on top changes too.
  let wasActive = false;
  createEffect(() => {
    const running = active();
    if (wasActive && !running) {
      void refetchMe();
      const outcome = view()?.report.last;
      if (outcome?.kind === "updated") setJustUpdated(outcome.version);
    }
    wasActive = running;
  });

  const canUpdate = (v: UpdatesView) =>
    v.daemon &&
    !v.stale &&
    !!v.report.available &&
    (v.report.phase === "idle" || v.report.phase === "prepared") &&
    !v.pending_request;

  const update = () =>
    attempt(
      async () => {
        setBusy(true);
        try {
          await platformApi.requestUpdate();
        } finally {
          setBusy(false);
          await refetch();
        }
      },
      setError,
      tr("platform:updates.requestSent"),
    );

  const save = () =>
    attempt(
      async () => {
        setBusy(true);
        try {
          await platformApi.setUpdatePolicy(policy()!);
          setPolicy(undefined);
        } finally {
          setBusy(false);
          await refetch();
          void refetchMe();
        }
      },
      setError,
      tr("platform:updates.policySaved"),
    );

  const edit = (change: (p: UpdatePolicy) => void) => {
    const p = structuredClone(policy()!);
    change(p);
    setPolicy(p);
  };

  return (
    <Page title={tr("platform:updates.updates")} subtitle={tr("platform:updates.intro")}>
      <A href="/settings" class="kn-back">
        <Icon name="back" size={16} stroke={1.5} />
        {tr("platform:updates.settings")}
      </A>
      <ErrorBox error={error() ?? view.error} />
      <Show
        when={view()}
        fallback={
          <Show when={!view.loading}>
            <div class="card kn-empty">{tr("platform:updates.notUpdatedHere")}</div>
          </Show>
        }
      >
        {(v) => (
          <>
            <Show when={!v().daemon}>
              <div class="kn-notice kn-notice--warn">
                El servicio de actualizaciones del servidor todavía no se reportó. Si la instalación
                es anterior a él, repite la instalación para tenerlo; mientras tanto se actualiza
                con <code>{tr("platform:updates.updateCommand")}</code>{" "}
                {tr("platform:updates.onTheServer")}
              </div>
            </Show>
            <Show when={v().stale}>
              <div class="kn-notice kn-notice--warn">
                {tr("platform:updates.serviceSilent", {
                  formatDate: formatDate(v().reported_at),
                })}
                <code>{tr("platform:updates.statusCommand")}</code>.
              </div>
            </Show>
            <Show when={v().report.warning}>
              {(w) => (
                <div class="kn-notice kn-notice--warn">
                  {tr("platform:updates.licenseCloud", { w: w() })}
                </div>
              )}
            </Show>

            <div class="card kn-form">
              <dl class="kn-dl">
                <dt>{tr("platform:updates.installedVersion")}</dt>
                <dd>{v().report.installed || "—"}</dd>
                <dt>{tr("platform:updates.status")}</dt>
                <dd>
                  {v().report.phase
                    ? PHASE_LABEL[v().report.phase as UpdatePhase]
                    : tr("platform:updates.noInformation")}
                  <Show when={v().report.target && v().report.phase !== "idle"}>
                    {" "}
                    ({v().report.target!.version})
                  </Show>
                </dd>
                <Show when={v().report.available}>
                  {(a) => (
                    <>
                      <dt>{tr("platform:updates.newVersion")}</dt>
                      <dd>
                        <strong>{a().version}</strong>
                        <Show when={v().report.failed.includes(a().version)}>
                          {tr("platform:updates.failedOnce")}
                        </Show>
                      </dd>
                      <Show when={a().notes.trim()}>
                        <dt>{tr("platform:updates.whatsNew")}</dt>
                        <dd>
                          <p class="kn-update__notes">{a().notes}</p>
                        </dd>
                      </Show>
                    </>
                  )}
                </Show>
                <Show when={v().maintenance}>
                  {(m) => (
                    <>
                      <dt>{tr("platform:updates.maintenance")}</dt>
                      <dd>
                        {tr("platform:updates.maintenanceSince", {
                          formatDate: formatDate(m().started_at),
                        })}
                      </dd>
                    </>
                  )}
                </Show>
                <Show when={v().pending_request}>
                  {(r) => (
                    <>
                      <dt>{tr("platform:updates.requested")}</dt>
                      <dd>
                        {tr("platform:updates.updateRequested", {
                          r: r().by ? `por ${r().by} ` : "",
                          formatDate: formatDate(r().at),
                        })}
                      </dd>
                    </>
                  )}
                </Show>
                <Show when={v().reported_at}>
                  <dt>{tr("platform:updates.lastReport")}</dt>
                  <dd>{formatDate(v().reported_at)}</dd>
                </Show>
              </dl>

              <Show
                when={
                  justUpdated()
                    ? {
                        percent: 100,
                        until: 100,
                        label: tr("platform:updates.updatedTo", {
                          justUpdated: justUpdated(),
                        }),
                        detail: tr("platform:updates.reloadPage"),
                        tone: "ok" as const,
                      }
                    : updateProgress(v(), now(), restarting())
                }
              >
                {(p) => (
                  <div class={`kn-update__progress kn-update__progress--${p().tone}`}>
                    <div class="kn-update__progress-head">
                      <strong>{p().label}</strong>
                      <span class="table__muted">{p().detail}</span>
                    </div>
                    <div
                      class="kn-update__bar"
                      role="progressbar"
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={p().percent}
                      aria-valuetext={p().label}
                    >
                      <div class="kn-update__bar-done" style={{ width: `${p().percent}%` }} />
                      <Show when={p().until > p().percent}>
                        <div
                          class="kn-update__bar-now"
                          style={{
                            left: `${p().percent}%`,
                            width: `${p().until - p().percent}%`,
                          }}
                        />
                      </Show>
                    </div>
                    <Show when={justUpdated()}>
                      <div class="kn-form__actions">
                        <button class="fk-btn kn-primary" onClick={() => location.reload()}>
                          {tr("platform:updates.reload")}
                        </button>
                      </div>
                    </Show>
                  </div>
                )}
              </Show>

              <Show when={IN_PROGRESS.includes(v().report.phase as UpdatePhase)}>
                <ol class="kn-update__steps">
                  <For each={STEPS}>
                    {(step, i) => {
                      const current = () => STEPS.findIndex((s) => s.phase === v().report.phase);
                      return (
                        <li
                          aria-current={step.phase === v().report.phase ? "step" : undefined}
                          classList={{ "kn-update__done": current() > i() }}
                        >
                          {step.label}
                        </li>
                      );
                    }}
                  </For>
                </ol>
              </Show>

              <Show when={v().report.last}>
                {(last) => (
                  <div
                    class={`kn-license__status kn-license__status--${OUTCOME_TONE[last().kind]}`}
                  >
                    Último intento ({formatDate(last().at)}): {OUTCOME_LABEL[last().kind]} ·{" "}
                    {last().detail}
                  </div>
                )}
              </Show>

              <div class="kn-form__actions">
                <button
                  class="fk-btn kn-primary"
                  disabled={busy() || !canUpdate(v())}
                  onClick={() => setConfirming(true)}
                >
                  {tr("platform:updates.updateNow")}
                </button>
              </div>
            </div>

            <h2 class="kn-section">{tr("platform:updates.whenUpdates")}</h2>
            <Show when={policy()}>
              {(p) => (
                <form
                  class="card kn-form"
                  onSubmit={(ev) => {
                    ev.preventDefault();
                    void save();
                  }}
                >
                  <For each={MODES}>
                    {(m) => (
                      <label class="kn-form__check">
                        <input
                          type="radio"
                          name="mode"
                          checked={p().mode === m.mode}
                          onChange={() => edit((x) => (x.mode = m.mode))}
                        />
                        <span>
                          <strong>{m.label}.</strong> {m.hint}
                        </span>
                      </label>
                    )}
                  </For>

                  <Show when={p().mode === "automatic"}>
                    <fieldset class="kn-update__days">
                      <legend>{tr("platform:updates.days")}</legend>
                      <For each={DAYS}>
                        {(day, i) => (
                          <label class="kn-form__check">
                            <input
                              type="checkbox"
                              checked={p().window.days.includes(i() + 1)}
                              onChange={(ev) =>
                                edit((x) => {
                                  const n = i() + 1;
                                  x.window.days = ev.currentTarget.checked
                                    ? [...x.window.days, n].sort((a, b) => a - b)
                                    : x.window.days.filter((d) => d !== n);
                                })
                              }
                            />
                            {day}
                          </label>
                        )}
                      </For>
                    </fieldset>
                    <div class="kn-form__row">
                      <label>
                        {tr("platform:updates.from")}
                        <input
                          type="time"
                          value={p().window.start}
                          onInput={(ev) => edit((x) => (x.window.start = ev.currentTarget.value))}
                          required
                        />
                      </label>
                      <label>
                        {tr("platform:updates.to")}
                        <input
                          type="time"
                          value={p().window.end}
                          onInput={(ev) => edit((x) => (x.window.end = ev.currentTarget.value))}
                          required
                        />
                      </label>
                      <label>
                        {tr("platform:updates.timeZone")}
                        <input
                          value={p().window.timezone}
                          onInput={(ev) =>
                            edit((x) => (x.window.timezone = ev.currentTarget.value.trim()))
                          }
                          required
                        />
                      </label>
                    </div>
                    <div class="kn-form__row">
                      <label>
                        {tr("platform:updates.scheduleGuard")}
                        <input
                          type="number"
                          min={0}
                          max={120}
                          value={minutes(p().skip_if_scheduled_within_secs)}
                          onInput={(ev) =>
                            edit(
                              (x) =>
                                (x.skip_if_scheduled_within_secs =
                                  Number(ev.currentTarget.value) * 60),
                            )
                          }
                        />
                      </label>
                    </div>
                    <label class="kn-form__check">
                      <input
                        type="checkbox"
                        checked={p().paused}
                        onChange={(ev) => edit((x) => (x.paused = ev.currentTarget.checked))}
                      />
                      {tr("platform:updates.pausedHelp")}
                    </label>
                  </Show>

                  <div class="kn-form__row">
                    <label>
                      {tr("platform:updates.drainMinutes")}
                      <input
                        type="number"
                        min={5}
                        max={240}
                        value={minutes(p().max_drain_secs)}
                        onInput={(ev) =>
                          edit((x) => (x.max_drain_secs = Number(ev.currentTarget.value) * 60))
                        }
                      />
                    </label>
                  </div>
                  <p class="table__muted">
                    {tr("platform:updates.drainHelp")}
                    <Show when={v().policy_updated_at}>
                      {tr("platform:updates.changedOn", {
                        formatDate: formatDate(v().policy_updated_at),
                        v: v().policy_updated_by ? ` por ${v().policy_updated_by}` : "",
                      })}
                    </Show>
                  </p>
                  <div class="kn-form__actions">
                    <button type="submit" class="fk-btn kn-primary" disabled={busy()}>
                      {tr("platform:updates.save")}
                    </button>
                  </div>
                </form>
              )}
            </Show>

            <h2 class="kn-section">{tr("platform:updates.history")}</h2>
            <Show
              when={v().history.length}
              fallback={<div class="card kn-empty">{tr("platform:updates.noUpdatesYet")}</div>}
            >
              <div class="card kn-table-wrap">
                <table class="table">
                  <thead>
                    <tr>
                      <th>{tr("platform:updates.date")}</th>
                      <th>{tr("platform:updates.result")}</th>
                      <th>{tr("platform:updates.version")}</th>
                      <th>{tr("platform:updates.details")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={v().history}>
                      {(h) => (
                        <tr>
                          <td>{formatDate(h.at)}</td>
                          <td>
                            <span
                              class={`kn-license__status kn-license__status--${OUTCOME_TONE[h.kind] ?? "warn"}`}
                            >
                              {OUTCOME_LABEL[h.kind] ?? h.kind}
                            </span>
                          </td>
                          <td class="kn-mono">
                            {h.from_version} → {h.version}
                          </td>
                          <td>{h.detail}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>

            <ConfirmDialog
              open={confirming()}
              title={`Actualizar a la ${v().report.available?.version ?? ""}`}
              message={tr("platform:updates.drainNotice", {
                maxDrainSecs: minutes(v().policy.max_drain_secs),
              })}
              confirmLabel={tr("platform:updates.update")}
              pending={busy()}
              onCancel={() => setConfirming(false)}
              onConfirm={() => {
                setConfirming(false);
                void update();
              }}
            />
          </>
        )}
      </Show>
    </Page>
  );
};
