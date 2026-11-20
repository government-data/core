import { For, type Component } from "solid-js";
import { formatNumber } from "@knime-orchestador/i18n";
import type { RuleTested } from "./api";
import { t } from "./i18n";

/**
 * A rule's failures by the shape of what it read (decision 0043, point 3):
 * each digit `9`, each letter `A`; how many of each and their share of the
 * failures, largest first. One chosen shows its records; chosen again, all
 * the failures read.
 */
export const FailGroups: Component<{
  r: RuleTested;
  chosen?: string;
  onChoose: (shape?: string) => void;
}> = (props) => {
  const share = (n: number) =>
    props.r.fails > 0 ? `${((n / props.r.fails) * 100).toFixed(1)}%` : "";
  const groups = () => props.r.groups ?? [];
  const shown = () => groups().reduce((sum, g) => sum + g.count, 0);
  return (
    <div class="qd-fail-groups">
      <p class="qd-hint">
        {t("quality:editor.test.groups", {
          groups: groups().length,
          shown: formatNumber(shown()),
          fails: formatNumber(props.r.fails),
        })}
      </p>
      <ul role="listbox" aria-label={t("quality:editor.test.groupsLabel")}>
        <For each={groups()}>
          {(g) => (
            <li
              role="option"
              aria-selected={props.chosen === g.shape}
              onClick={() => props.onChoose(props.chosen === g.shape ? undefined : g.shape)}
            >
              <span class="qd-mono qd-fail-groups__shape">{g.shape}</span>
              <span class="qd-fail-groups__count">{formatNumber(g.count)}</span>
              <span class="qd-hint">{share(g.count)}</span>
              <span
                class="qd-fail-groups__bar"
                style={{ width: share(g.count) }}
                aria-hidden="true"
              />
            </li>
          )}
        </For>
      </ul>
    </div>
  );
};
