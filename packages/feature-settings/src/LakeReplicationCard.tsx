import { createResource, For, Show, type Component } from "solid-js";
import { formatDateTime } from "@knime-orchestador/i18n";
import { SettingsApiError } from "./api";
import { t } from "./i18n";
import { readinessLine, warehouseLine } from "./lakeReplication";
import type { MachinesApi } from "./machinesApi";

/** The replication's readiness and each warehouse's last run, read again on demand. */
export const LakeReplicationCard: Component<{ api: MachinesApi; asked: number }> = (props) => {
  const [state, { refetch }] = createResource(
    () => props.asked,
    () => props.api.replication(),
  );
  return (
    <div class="card">
      <h2 class="st-subtitle">{t("settings:lake.replication.title")}</h2>
      <Show when={state.latest}>
        {(r) => (
          <p>
            {t("settings:lake.replication.intro", {
              days: r().retention_days,
              noncurrent: r().noncurrent_days,
            })}
          </p>
        )}
      </Show>
      <Show when={state.error}>
        <p class="st-notice st-notice--error">
          {state.error instanceof SettingsApiError
            ? state.error.message
            : t("settings:lake.replication.loadError")}
        </p>
      </Show>
      <Show when={state.latest}>
        {(r) => {
          const line = () => readinessLine(r());
          const tone = () => (line().tone === "ok" ? "" : ` st-notice--${line().tone}`);
          return (
            <>
              <p class={`st-notice${tone()}`}>{line().text}</p>
              <Show
                when={r().warehouses.length > 0}
                fallback={<p class="table__muted">{t("settings:lake.replication.none")}</p>}
              >
                <table class="table">
                  <thead>
                    <tr>
                      <th>{t("settings:lake.replication.warehouse")}</th>
                      <th>{t("settings:lake.replication.state")}</th>
                      <th>{t("settings:lake.replication.checked")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={r().warehouses}>
                      {(w) => (
                        <tr>
                          <td>{w.warehouse}</td>
                          <td>{warehouseLine(w).text}</td>
                          <td class="table__muted">{formatDateTime(w.checked_at * 1000)}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </Show>
            </>
          );
        }}
      </Show>
      <div class="st-actions">
        <button type="button" class="fk-btn" disabled={state.loading} onClick={() => refetch()}>
          {state.loading ? t("settings:lake.replica.checking") : t("settings:lake.replica.check")}
        </button>
      </div>
    </div>
  );
};
