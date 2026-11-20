import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { Modal, Status } from "@knime-orchestador/feature-kit/ui";
import { editsApi, QualityError, rulesApi, type FindingOut } from "./api";
import { review } from "./deployCheck";
import { diffMode, DiffModes, DiffView } from "./DiffView";
import { rulesChanged } from "./explorerCache";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";

/** What is to go to `main`: the file, its new text and `main`'s as it is. */
export interface Deploying {
  dataset: string;
  file: string;
  yaml: string;
  /** `main`'s file now; `null` if it has none. */
  expected: string | null;
  /** The dialog's title: a deploy, a restore or an undo. */
  title: string;
}

/** The text key of a refused deploy. */
function refusal(err: unknown): string {
  const status = err instanceof QualityError ? err.status : 0;
  if (status === 409) return "quality:editor.stale";
  if (status === 422) return "quality:deploy.lint";
  return "quality:common.saveError";
}

/** Findings of the linter, each with its line and code. */
const Findings: Component<{ findings: FindingOut[]; tone: "critical" | "warning" }> = (props) => (
  <ul class="qd-history-list">
    <For each={props.findings}>
      {(f) => (
        <li>
          <Status tone={props.tone} label={f.code} />{" "}
          <Show when={f.line}>
            <span class="qd-mono qd-hint">{t("quality:deploy.line", { line: f.line })}</span>{" "}
          </Show>
          {f.message}
        </li>
      )}
    </For>
  </ul>
);

/** Which rules it adds, removes and changes, by their ids. */
const RuleChanges: Component<{ added: string[]; removed: string[]; changed: string[] }> = (
  props,
) => (
  <ul class="qd-history-list">
    <For
      each={[
        ["added", props.added] as const,
        ["removed", props.removed] as const,
        ["changed", props.changed] as const,
      ]}
    >
      {([kind, ids]) => (
        <Show when={ids.length > 0}>
          <li>
            {t(`quality:deploy.${kind}`, { count: ids.length })}{" "}
            <span class="qd-mono">{ids.join(", ")}</span>
          </li>
        </Show>
      )}
    </For>
  </ul>
);

/**
 * Before a deploy, a restore or an undo reaches `main` (decision 0051):
 * the linter reads the file to deploy and `main`'s, and says what blocks
 * it (its errors, as the API would refuse them), what to look at (its
 * warnings), which rules it adds, removes or changes, and the change line
 * by line; _Deploy_ only when nothing blocks it.
 */
export const DeployConfirm: Component<{
  deploying: Deploying;
  onClose: () => void;
  onDone: (text: string, ok: boolean) => void;
}> = (props) => {
  const d = props.deploying;
  const [checked] = createResource(async () => {
    const after = await rulesApi.check(d.dataset, d.yaml, d.file);
    const before =
      d.expected === null ? undefined : await rulesApi.check(d.dataset, d.expected, d.file);
    return review(before, after);
  });
  const [busy, setBusy] = createSignal(false);
  const deploy = async () => {
    setBusy(true);
    try {
      await editsApi.deploy(d.dataset, d.file, d.yaml, d.expected);
      rulesChanged(d.dataset);
      props.onDone(t("quality:editor.deployed"), true);
    } catch (err) {
      props.onDone(t(refusal(err)), false);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal open size="wide" title={d.title} onClose={props.onClose}>
      <LoadFailed when={checked.error} />
      <Show when={checked()} fallback={<p class="qd-hint">{t("quality:deploy.checking")}</p>}>
        {(r) => (
          <div class="qd-column-history qd-modal-scroll">
            <Show when={r().errors.length > 0}>
              <h3 class="qd-section">{t("quality:deploy.errors", { count: r().errors.length })}</h3>
              <Findings findings={r().errors} tone="critical" />
            </Show>
            <Show when={r().warnings.length > 0}>
              <h3 class="qd-section">
                {t("quality:deploy.warnings", { count: r().warnings.length })}
              </h3>
              <Findings findings={r().warnings} tone="warning" />
            </Show>
            <Show when={!r().errors.length && !r().warnings.length}>
              <p class="qd-ok">{t("quality:deploy.clean")}</p>
            </Show>
            <h3 class="qd-section">{t("quality:deploy.rules")}</h3>
            <Show
              when={r().added.length + r().removed.length + r().changed.length > 0}
              fallback={<p class="qd-hint">{t("quality:deploy.noRuleChanges")}</p>}
            >
              <RuleChanges added={r().added} removed={r().removed} changed={r().changed} />
            </Show>
            <DiffModes />
            <DiffView
              change={{ file: d.file, before: d.expected, after: d.yaml }}
              mode={diffMode()}
            />
            <div class="qd-actions qd-sticky-actions">
              <button type="button" class="fk-btn" onClick={props.onClose}>
                {t("quality:common.cancel")}
              </button>
              <button
                type="button"
                class="fk-btn fk-btn--primary"
                disabled={busy() || r().errors.length > 0}
                title={r().errors.length ? t("quality:editor.blocked") : undefined}
                onClick={() => void deploy()}
              >
                {t("quality:editor.deploy")}
              </button>
            </div>
          </div>
        )}
      </Show>
    </Modal>
  );
};
