import { For, Show, type Component } from "solid-js";
import type { IamPolicy, IamStatement } from "./api";
import { t } from "./i18n";
import { actionLabel, policyDescription } from "./iam";

/** A statement in one line: «Permitir: Ver reglas, Ejecutar pipeline · en *». */
function summary(s: IamStatement): string {
  const verb = s.effect === "Allow" ? t("settings:policies.allow") : t("settings:policies.deny");
  const actions = s.actions.includes("*")
    ? t("settings:policies.everything")
    : s.actions.map(actionLabel).join(", ");
  return `${verb}: ${actions} · ${t("settings:policies.on")} ${s.resources.join(", ")}`;
}

/**
 * The policies of Settings → Policies, as a table: the managed ones (the
 * application's and the modules', a removed module's marked) and the custom
 * ones, which can be changed and deleted.
 */
export const PoliciesTable: Component<{
  policies: IamPolicy[];
  busy: boolean;
  /** Whether the catalog is loaded, so a policy can be changed. */
  editable: boolean;
  onEdit: (p: IamPolicy) => void;
  onDelete: (p: IamPolicy) => void;
}> = (props) => (
  <div class="card">
    <table class="table">
      <thead>
        <tr>
          <th>{t("settings:policies.name")}</th>
          <th>{t("settings:policies.statements")}</th>
          <th>{t("settings:policies.attached")}</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <For each={props.policies}>
          {(p) => (
            <tr>
              <td>
                <div>
                  {p.name}{" "}
                  <Show when={p.managed}>
                    <span class="badge badge--muted">{t("settings:iam.managedBadge")}</span>
                  </Show>{" "}
                  <Show when={p.module && !p.module.installed}>
                    <span class="badge badge--muted">
                      {t("settings:policies.removedModule", { name: p.module?.name })}
                    </span>
                  </Show>
                </div>
                <Show when={policyDescription(p)}>
                  <div class="table__muted">{policyDescription(p)}</div>
                </Show>
              </td>
              <td>
                <For each={p.statements}>{(s) => <div class="table__muted">{summary(s)}</div>}</For>
              </td>
              <td>{p.attached}</td>
              <td class="st-linked__actions">
                <Show when={!p.managed}>
                  <button
                    type="button"
                    class="fk-btn fk-btn--sm"
                    disabled={props.busy || !props.editable}
                    onClick={() => props.onEdit(p)}
                  >
                    {t("settings:users.edit")}
                  </button>{" "}
                  <button
                    type="button"
                    class="fk-btn fk-btn--sm fk-btn--danger"
                    disabled={props.busy}
                    onClick={() => props.onDelete(p)}
                  >
                    {t("settings:users.delete")}
                  </button>
                </Show>
              </td>
            </tr>
          )}
        </For>
      </tbody>
    </table>
  </div>
);
