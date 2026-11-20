import { createResource, Show, type Component } from "solid-js";
import { A } from "@solidjs/router";
import { may } from "./api";
import { who } from "./explorerCache";
import { t } from "./i18n";
import { fieldFile } from "./newRule";

/**
 * _+_ beside a field of the tree, for whoever may edit the rules: opens
 * the field's own rules file (decision 0044), new if `main` lacks it, with
 * a rule for it at its end (`?add=`); nothing is saved until it is saved
 * as a draft.
 */
export const AddRule: Component<{ dataset: string; field: string }> = (props) => {
  const [me] = createResource(who);
  const file = () => fieldFile(props.field);
  return (
    <Show when={file() && may(me(), "quality:EditRules")}>
      <A
        class="qd-tree-add"
        title={t("quality:explorer.addRule", { field: props.field })}
        aria-label={t("quality:explorer.addRule", { field: props.field })}
        href={`/quality/rules/${encodeURIComponent(props.dataset)}/${encodeURIComponent(file()!)}?add=${encodeURIComponent(props.field)}`}
      >
        +
      </A>
    </Show>
  );
};
