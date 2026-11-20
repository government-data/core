import { createSignal, Show, type Component } from "solid-js";
import { may } from "./api";
import { DeployConfirm, type Deploying } from "./DeployConfirm";
import type { RuleLeaf } from "./explorer";
import { knownMe, rulesOf } from "./explorerCache";
import { t } from "./i18n";
import { notify, warn } from "./Notify";
import { withoutRule } from "./removeRule";
import type { MenuItem } from "./TreeMenu";

const [deploying, setDeploying] = createSignal<{ rule: string; d: Deploying }>();

/** The rule taken out of its file on `main`, to deploy once confirmed. */
async function ask(dataset: string, rule: RuleLeaf) {
  const out = await rulesOf(dataset).catch(() => undefined);
  const file = out?.files.find((f) => f.file === rule.file);
  const yaml = file && withoutRule(file.yaml, rule.id);
  if (!file || yaml === undefined)
    return warn(t("quality:rules.deleteRule.cannot", { rule: rule.id }));
  setDeploying({
    rule: rule.id,
    d: {
      dataset,
      file: rule.file,
      yaml,
      expected: file.yaml,
      title: t("quality:rules.deleteRule.title", { rule: rule.id }),
    },
  });
}

/**
 * _Delete rule_ in a rule's menu (decision 0051): for whoever may deploy,
 * its file without it deployed to `main` as any deploy, read by the linter
 * and confirmed first, and undone from the field's history.
 */
export function ruleMenu(dataset: string, rule: RuleLeaf): MenuItem[] {
  const me = knownMe();
  if (!may(me, "quality:ApproveRules") || !may(me, "quality:ManageDatasets")) return [];
  return [{ label: t("quality:rules.deleteRule.action"), run: () => void ask(dataset, rule) }];
}

/** The deploy that deletes a rule, once, outside the tree. */
export const RuleDeleteHost: Component = () => (
  <Show when={deploying()} keyed>
    {(x) => (
      <DeployConfirm
        deploying={x.d}
        onClose={() => setDeploying(undefined)}
        onDone={(text, ok) => {
          setDeploying(undefined);
          notify(ok ? t("quality:rules.deleteRule.done", { rule: x.rule }) : text, !ok);
        }}
      />
    )}
  </Show>
);
