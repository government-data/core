import { For, Show, type Component } from "solid-js";
import { ruleMenu } from "./RuleDelete";
import { openMenu } from "./TreeMenu";
import { A } from "@solidjs/router";
import { formatNumber } from "@knime-orchestador/i18n";
import { AddRule } from "./AddRule";
import { Branch } from "./Branch";
import {
  propertiesPath,
  type DerivedNode,
  type Mark,
  type RuleLeaf,
  type TableNode,
} from "./explorer";
import { t } from "./i18n";
import { lastTested } from "./ruleTest";

/*
 * The explorer's leaves (decision 0043): a node's mark, a rule, the link
 * to a table's or column's properties, a column no field reads and a
 * dataset's derived fields.
 */

/** A mark beside a name; a node read by a mapping has none. */
export const MarkOf: Component<{ mark: Mark }> = (props) => (
  <Show when={props.mark !== "read"}>
    <small class={`qd-mark qd-mark--${props.mark}`}>
      {t(`quality:explorer.mark.${props.mark}`)}
    </small>
  </Show>
);

/** A rule, opened at its line in the editor; the other fields it reads beside it. */
export const RuleItem: Component<{ dataset: string; rule: RuleLeaf }> = (props) => (
  <li
    role="treeitem"
    class="qd-tree-rule"
    onContextMenu={(e) => openMenu(e, ruleMenu(props.dataset, props.rule))}
  >
    <A
      class="qd-link qd-mono"
      title={t("quality:editor.open")}
      href={`/quality/rules/${encodeURIComponent(props.dataset)}/${encodeURIComponent(props.rule.file)}?rule=${encodeURIComponent(props.rule.id)}`}
    >
      {props.rule.id}
    </A>
    <Show when={lastTested(props.dataset, props.rule.id)}>
      {(last) => (
        <Show
          when={last().fails > 0}
          fallback={
            <small class="qd-tested qd-tested--passes" title={t("quality:explorer.tested.passes")}>
              {" "}
              ✓
            </small>
          }
        >
          <small class="qd-tested qd-tested--fails">
            {" "}
            {t("quality:explorer.tested.fails", { n: formatNumber(last().fails) })}
          </small>
        </Show>
      )}
    </Show>
    <Show when={props.rule.also.length}>
      <small class="qd-hint">
        {" "}
        {t("quality:explorer.also", { fields: props.rule.also.join(", ") })}
      </small>
    </Show>
  </li>
);

/** Opens the properties of a table, or of a column among them, in the work area. */
export const Info: Component<{ source?: string; table: TableNode; column?: string }> = (props) => (
  <A
    class="qd-tree-info"
    title={t("quality:explorer.props.open")}
    aria-label={t("quality:explorer.props.open")}
    href={propertiesPath({
      source: props.source,
      schema: props.table.schema,
      table: props.table.name,
      column: props.column,
    })}
  >
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="10" />
      <path d="M12 16v-5M12 8h.01" />
    </svg>
  </A>
);

/** A column, nothing under it: dimmed unless a field reads it. */
export const ColumnLeaf: Component<{
  name: string;
  dataType?: string;
  source?: string;
  table: TableNode;
  /** A dataset's field reads it. */
  read?: boolean;
}> = (props) => (
  <li role="treeitem" class={`qd-tree-column qd-tree--${props.read ? "read" : "unread"}`}>
    <span class="qd-mono">{props.name}</span>{" "}
    <Info source={props.source} table={props.table} column={props.name} />{" "}
    <small class="qd-hint">{props.dataType}</small>
  </li>
);

/** A dataset's derived fields, apart, each with its rules. */
export const DerivedFields: Component<{ id: string; dataset: string; fields: DerivedNode[] }> = (
  props,
) => (
  <Branch id={props.id} class="qd-tree-derived" label={<em>{t("quality:explorer.derived")}</em>}>
    {() => (
      <For each={props.fields}>
        {(field) => (
          <Branch
            id={`${props.id}/${field.field}`}
            label={
              <span>
                <span class="qd-mono">{field.field}</span>{" "}
                <small class="qd-hint">
                  {t("quality:explorer.from", { fields: field.from.join(", ") })}
                </small>{" "}
                <AddRule dataset={props.dataset} field={field.field} />
              </span>
            }
          >
            {() => (
              <For each={field.rules}>
                {(rule) => <RuleItem dataset={props.dataset} rule={rule} />}
              </For>
            )}
          </Branch>
        )}
      </For>
    )}
  </Branch>
);
