import { createResource, createSignal, Show, type Component } from "solid-js";
import { editsApi, may, QualityError } from "./api";
import { mappingsChanged, who } from "./explorerCache";
import type { NewDataset } from "./explorer";
import { t } from "./i18n";
import { warn } from "./Notify";

/** Why the table was not mapped, in words (the API's `error` and values). */
function refusal(err: unknown): string {
  const body =
    err instanceof QualityError ? (err.body as Record<string, string> | undefined) : undefined;
  if (body?.error === "field") return t("quality:mapTable.refused.taken", body);
  if (body?.error && ["mapped", "mapping", "dataset_taken"].includes(body.error))
    return t(`quality:mapTable.refused.${body.error}`);
  return t("quality:common.saveError");
}

/**
 * _Map the whole table_ (decisions 0043 and 0047), for whoever may manage
 * the dataset: every column no field reads becomes a field of its mapping,
 * at once, on `main`; a table no dataset reads starts one (`start`). The
 * tree asks for the datasets again, so each field shows with its _+_.
 */
export const MapTable: Component<{
  dataset: string;
  columns: { name: string; dataType?: string }[];
  start?: NewDataset["table"];
}> = (props) => {
  const [me] = createResource(who);
  const [busy, setBusy] = createSignal(false);
  const run = async () => {
    setBusy(true);
    try {
      const columns = props.columns.map((c) => ({ column: c.name, type: c.dataType ?? "" }));
      await editsApi.addFields(props.dataset, { columns, table: props.start });
      mappingsChanged();
    } catch (err) {
      warn(refusal(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Show when={props.columns.length && may(me(), "quality:ManageDatasets")}>
      {" "}
      <button
        type="button"
        class="fk-btn fk-btn--sm"
        disabled={busy()}
        title={
          props.start
            ? t("quality:mapTable.starts", { dataset: props.dataset })
            : t("quality:mapTable.hint", { count: props.columns.length })
        }
        onClick={() => void run()}
      >
        {t("quality:mapTable.action")}
      </button>
    </Show>
  );
};
