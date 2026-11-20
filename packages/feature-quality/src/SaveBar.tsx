import { createSignal, Show, type Component } from "solid-js";
import { editsApi, QualityError, type FindingOut } from "./api";
import { DeployConfirm, type Deploying } from "./DeployConfirm";
import { draftsChanged } from "./explorerCache";
import { t } from "./i18n";
import { notify } from "./Notify";

/** The text key of a refused save or deploy, by its status and findings. */
function refusal(err: unknown, findings: boolean): string {
  if (findings) return "quality:editor.notSaved";
  const status = err instanceof QualityError ? err.status : 0;
  const code = err instanceof QualityError ? (err.body as { error?: string })?.error : undefined;
  // `main`'s file changed since it was opened (0051), or the draft is
  // another column's (0050).
  if (status === 409) return code === "stale" ? "quality:editor.stale" : "quality:editor.otherFile";
  return "quality:common.saveError";
}

/**
 * _Deploy_ (decision 0051): for whoever may approve the rules, the file
 * opened from `main` goes to `main`, one commit, and its rules are under
 * its field at once, once the linter's reading of it is confirmed
 * (`DeployConfirm`); refused if `main`'s file changed since it was opened.
 * Otherwise, and on a draft, it is saved as the column's draft (0021,
 * 0050), reviewed from the column's ⎇. A file with lint errors is neither;
 * the API's errors come back to the panel.
 */
export const SaveBar: Component<{
  dataset: string;
  file: string;
  draft?: string;
  /** May deploy to `main`. */
  deploys: boolean;
  /** `main`'s file as it was opened; `null` if it had none. */
  expected: string | null;
  text: string;
  changed: boolean;
  blocked: boolean;
  onSaved: (draft: string) => void;
  onDeployed: () => void;
  onRefused: (findings: FindingOut[]) => void;
}> = (props) => {
  const [pending, setPending] = createSignal(false);
  const deploying = () => props.deploys && !props.draft;
  const [confirming, setConfirming] = createSignal<Deploying>();

  const save = async () => {
    setPending(true);
    try {
      if (deploying()) {
        // Read by the linter and confirmed first (0051).
        setConfirming({
          dataset: props.dataset,
          file: props.file,
          yaml: props.text,
          expected: props.expected,
          title: t("quality:deploy.title", { file: props.file }),
        });
        return;
      }
      const out = props.draft
        ? await editsApi.save(props.dataset, props.draft, props.file, props.text)
        : await editsApi.start(props.dataset, props.file, props.text);
      notify(t("quality:editor.saved"));
      draftsChanged();
      props.onSaved(out.id);
    } catch (err) {
      const body =
        err instanceof QualityError ? (err.body as { findings?: FindingOut[] }) : undefined;
      if (body?.findings) props.onRefused(body.findings);
      notify(t(refusal(err, !!body?.findings)), true);
    } finally {
      setPending(false);
    }
  };

  const label = () =>
    deploying()
      ? "quality:editor.deploy"
      : props.draft
        ? "quality:editor.saveDraft"
        : "quality:editor.startDraft";

  return (
    <div class="qd-actions qd-savebar">
      <button
        type="button"
        class="fk-btn fk-btn--primary"
        disabled={pending() || !props.changed || props.blocked}
        title={props.blocked ? t("quality:editor.blocked") : undefined}
        onClick={() => void save()}
      >
        {t(label())}
      </button>
      <Show when={confirming()} keyed>
        {(d) => (
          <DeployConfirm
            deploying={d}
            onClose={() => setConfirming(undefined)}
            onDone={(text, ok) => {
              setConfirming(undefined);
              notify(text, !ok);
              if (ok) props.onDeployed();
            }}
          />
        )}
      </Show>
    </div>
  );
};
