import { For, Show, type Component } from "solid-js";
import type { IamPolicy } from "./api";
import { t } from "./i18n";

/**
 * What the IAM screens share (ADR 0016): the names of the actions and of the
 * managed policies in the UI's language, and the list of policies to tick.
 */

/** «Ver reglas» for `quality:ViewRules` (or the name as is). */
export function actionLabel(action: string): string {
  const key = `settings:iam.action.${action.replace(":", "_")}`;
  const label = t(key);
  return label === key || label === key.slice("settings:".length) ? action : label;
}

/** A managed policy's description in the UI's language; a module's comes in
 * it; a custom one's, as written. */
export function policyDescription(
  p: Pick<IamPolicy, "name" | "description" | "managed" | "module">,
): string {
  return p.managed && !p.module ? t(`settings:iam.managed.${p.name}`) : p.description;
}

/** The policies, as chips. */
export const PolicyChips: Component<{ names: string[]; empty?: string }> = (props) => (
  <Show
    when={props.names.length > 0}
    fallback={<span class="table__muted">{props.empty ?? t("settings:iam.none")}</span>}
  >
    <div class="st-access-chips">
      <For each={props.names}>{(n) => <span class="badge badge--muted">{n}</span>}</For>
    </div>
  </Show>
);

/** The policies to tick, each with what it is for. */
export const PolicyChecks: Component<{
  policies: IamPolicy[];
  chosen: string[];
  onChange: (chosen: string[]) => void;
  disabled?: boolean;
}> = (props) => (
  <div class="st-policy-checks">
    <For each={props.policies}>
      {(p) => (
        <label class="st-check st-check--block">
          <input
            type="checkbox"
            checked={props.chosen.includes(p.name)}
            disabled={props.disabled}
            onChange={(e) =>
              props.onChange(
                e.currentTarget.checked
                  ? [...props.chosen, p.name]
                  : props.chosen.filter((n) => n !== p.name),
              )
            }
          />
          <span>
            <strong>{p.name}</strong>
            <Show when={p.managed}>
              {" "}
              <span class="badge badge--muted">{t("settings:iam.managedBadge")}</span>
            </Show>
            <Show when={policyDescription(p)}>
              <span class="table__muted st-check__detail">{policyDescription(p)}</span>
            </Show>
          </span>
        </label>
      )}
    </For>
  </div>
);
