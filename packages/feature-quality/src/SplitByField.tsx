import { createResource, createSignal, Show, type Component } from "solid-js";
import { editsApi, may, QualityError } from "./api";
import { draftsChanged, who } from "./explorerCache";
import { t } from "./i18n";
import { notify, warn } from "./Notify";

/** Why the files were not split, in words (the API's `reason` and values). */
function refusal(err: unknown): string {
  const body =
    err instanceof QualityError ? (err.body as Record<string, string> | undefined) : undefined;
  if (body?.error === "manage") return t("quality:split.manage");
  if (body?.error !== "split") return t("quality:common.saveError");
  return t(`quality:split.refused.${body.reason}`, body);
}

/**
 * _Split by field_ on a dataset (decisions 0044, 0045), for whoever may
 * edit its rules: its files on `main` as its global rules and a file per
 * field, on a new draft to review; nothing changes `main`.
 */
export const SplitByField: Component<{ dataset: string }> = (props) => {
  const [me] = createResource(who);
  const [busy, setBusy] = createSignal(false);
  const run = async () => {
    setBusy(true);
    try {
      const saved = await editsApi.split(props.dataset);
      if (saved) draftsChanged();
      if (saved) notify(t("quality:split.saved"));
      else warn(t("quality:split.already"));
    } catch (err) {
      warn(refusal(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={may(me(), "quality:EditRules")}>
      {" "}
      <button
        type="button"
        class="fk-btn fk-btn--sm"
        disabled={busy()}
        title={t("quality:split.hint")}
        onClick={() => void run()}
      >
        {t("quality:split.action")}
      </button>
    </Show>
  );
};
