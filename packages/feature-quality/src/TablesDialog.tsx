import { createResource, createSignal, For, onCleanup, Show, type Component } from "solid-js";
import { formatNumber } from "@knime-orchestador/i18n";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { sourcesApi } from "./api";
import { ListingStatus } from "./ListingStatus";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { filterTables, onItsWay, tableRef } from "./sources";

/** How often a listing on its way is asked again, and for how long: its
 * retries wait a minute, then two (the queue's backoff). */
const POLL_MS = 2000;
const POLL_FOR_MS = 600_000;

/**
 * A source's tables (decision 0042): its last listing, read by a job when
 * the source was saved or on _Refresh_, searchable; picking one maps it
 * (0038). A table it does not show can still be typed.
 */
export const TablesDialog: Component<{
  source: string;
  manages: boolean;
  onPick: (table: string) => void;
  onClose: () => void;
}> = (props) => {
  const [listing, { refetch }] = createResource(() => props.source, sourcesApi.tables);
  const [query, setQuery] = createSignal("");
  const [refused, setRefused] = createSignal(false);
  let since = Date.now();
  const timer = setInterval(() => {
    if (onItsWay(listing()) && Date.now() - since < POLL_FOR_MS) void refetch();
  }, POLL_MS);
  onCleanup(() => clearInterval(timer));

  const refresh = async () => {
    setRefused(false);
    try {
      await sourcesApi.listTables(props.source);
      since = Date.now();
      void refetch();
    } catch {
      setRefused(true);
    }
  };

  return (
    <Modal
      open
      title={t("quality:sources.tables.title", { name: props.source })}
      onClose={props.onClose}
    >
      <div class="qd-form">
        <LoadFailed when={listing.error} />
        <Show when={listing()}>{(l) => <ListingStatus listing={l()} />}</Show>
        <Show when={refused()}>
          <p class="qd-error">{t("quality:sources.tables.refused")}</p>
        </Show>
        <Show when={(listing()?.tables.length ?? 0) > 0}>
          <label class="qd-field">
            <span>{t("quality:sources.tables.search")}</span>
            <input
              class="qd-mono"
              type="search"
              autocomplete="off"
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
            />
          </label>
          <ul class="qd-table-list">
            <For
              each={filterTables(listing()?.tables ?? [], query())}
              fallback={<li class="qd-hint">{t("quality:sources.tables.noMatch")}</li>}
            >
              {(table) => (
                <li>
                  <button type="button" onClick={() => props.onPick(tableRef(table))}>
                    <span class="qd-mono">{tableRef(table)}</span>
                    <small>
                      {t("quality:sources.tables.columns", {
                        count: table.columns.length,
                        n: formatNumber(table.columns.length),
                      })}
                    </small>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <div class="qd-actions">
          <button type="button" class="fk-btn" onClick={() => props.onPick("")}>
            {t("quality:sources.tables.type")}
          </button>
          <Show when={props.manages}>
            <button
              type="button"
              class="fk-btn"
              disabled={onItsWay(listing())}
              onClick={() => void refresh()}
            >
              {t("quality:sources.tables.refresh")}
            </button>
          </Show>
        </div>
      </div>
    </Modal>
  );
};
