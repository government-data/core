import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { draftsApi } from "./api";
import { diffMode, DiffModes, DiffView } from "./DiffView";
import { authorName } from "./drafts";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";

const short = (hash: string) => hash.slice(0, 7);

/**
 * A draft read as the branch it is (decision 0021): its commits on a line,
 * newest first, down to where it left `main`; the whole draft or one
 * commit chosen, what it changed in each file.
 */
export const DraftHistory: Component<{ dataset: string; id: string }> = (props) => {
  const [history] = createResource(
    () => ({ dataset: props.dataset, id: props.id }),
    (k) => draftsApi.history(k.dataset, k.id),
  );
  // `undefined`: the whole draft against where it left `main`.
  const [chosen, setChosen] = createSignal<string>();
  const [changes] = createResource(
    () => ({ dataset: props.dataset, id: props.id, commit: chosen() ?? "" }),
    (k) => draftsApi.changes(k.dataset, k.id, k.commit || undefined),
  );
  return (
    <div class="qd-history">
      <LoadFailed when={history.error} />
      <Show when={history()}>
        {(h) => (
          <ol class="qd-graph" aria-label={t("quality:drafts.history.title")}>
            <li>
              <button
                type="button"
                class="qd-graph__node qd-graph__node--all"
                aria-pressed={chosen() === undefined}
                onClick={() => setChosen(undefined)}
              >
                <span class="qd-graph__dot" />
                <span>{t("quality:drafts.history.whole")}</span>
              </button>
            </li>
            <For each={h().commits}>
              {(c) => (
                <li>
                  <button
                    type="button"
                    class="qd-graph__node"
                    aria-pressed={chosen() === c.commit}
                    onClick={() => setChosen(c.commit)}
                  >
                    <span class="qd-graph__dot" />
                    <span class="qd-graph__message">{c.message}</span>
                    <span class="qd-hint" title={c.author}>
                      {authorName(c.author)} · {c.at.slice(0, 16)}
                    </span>
                    <span class="qd-mono qd-code">{short(c.commit)}</span>
                  </button>
                </li>
              )}
            </For>
            <li class="qd-graph__node qd-graph__node--base">
              <span class="qd-graph__dot" />
              <span>{t("quality:drafts.history.base")}</span>
              <span class="qd-mono qd-code">{short(h().base)}</span>
            </li>
          </ol>
        )}
      </Show>
      <div class="qd-history__changes">
        <DiffModes />
        <LoadFailed when={changes.error} />
        <Show when={changes()}>
          {(c) => (
            <Show
              when={c().files.length > 0}
              fallback={<p class="qd-hint">{t("quality:drafts.history.none")}</p>}
            >
              <For each={c().files}>{(f) => <DiffView change={f} mode={diffMode()} />}</For>
            </Show>
          )}
        </Show>
      </div>
    </div>
  );
};
