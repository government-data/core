import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { draftsApi, may, type MainChange } from "./api";
import { DeployConfirm, type Deploying } from "./DeployConfirm";
import { diffMode, DiffModes, DiffView } from "./DiffView";
import { authorName, doltDate } from "./drafts";
import { DraftReview } from "./DraftReview";
import { drafted, draftsChanged, draftsOf, ruled, who, type OpenDraft } from "./explorerCache";
import { t } from "./i18n";
import { LoadFailed, notify } from "./Notify";
import { BranchIcon } from "./TestIcons";
import { emptyFile } from "./newRule";

const short = (hash: string) => hash.slice(0, 7);

/** Where a column is: its dataset and its rules file; none for the dataset itself. */
type Column = { dataset: string; file?: string; label: string };

/** The drafts of `column`: a column's own, or the dataset's without a file. */
const draftsFor = (all: OpenDraft[] | undefined, file?: string) =>
  (all ?? []).filter((d) => d.file === file);

/**
 * A change on `main`: who, when and why, and, opened, the file before and
 * after; for whoever may deploy, _Restore this version_ and, on the latest,
 * _Undo_ (decision 0051), each a deploy over the current file, read by
 * the linter and confirmed first.
 */
const MainChangeItem: Component<{
  change: MainChange;
  column: Column;
  /** `main`'s file now, if this is a rules file whoever looks may deploy. */
  current?: string | null;
  latest: boolean;
}> = (props) => {
  const [confirming, setConfirming] = createSignal<Deploying>();
  // Read by the linter and confirmed before it reaches `main` (0051).
  const put = (yaml: string, title: string) =>
    setConfirming({
      dataset: props.column.dataset,
      file: props.column.file ?? "",
      yaml,
      expected: props.current ?? null,
      title,
    });
  const busy = () => !!confirming();
  return (
    <li>
      <details class="qd-file">
        <summary>
          <span class="qd-mono qd-hint">{short(props.change.commit.commit)}</span>{" "}
          <span title={props.change.commit.author}>{authorName(props.change.commit.author)}</span>{" "}
          <small class="qd-hint">{doltDate(props.change.commit.at)}</small>{" "}
          <span>{props.change.commit.message}</span>
        </summary>
        <Show when={props.current !== undefined}>
          <div class="qd-actions">
            <Show when={!props.latest && props.change.after}>
              {(after) => (
                <button
                  type="button"
                  class="fk-btn fk-btn--sm"
                  disabled={busy()}
                  onClick={() => put(after(), t("quality:history.restore"))}
                >
                  {t("quality:history.restore")}
                </button>
              )}
            </Show>
            <Show when={props.latest}>
              {/* Undoing the file's first deploy leaves it with no rules. */}
              <button
                type="button"
                class="fk-btn fk-btn--sm"
                disabled={busy()}
                onClick={() =>
                  put(
                    props.change.before ?? emptyFile(props.column.dataset, props.column.file),
                    t("quality:history.undo"),
                  )
                }
              >
                {t("quality:history.undo")}
              </button>
            </Show>
            <Show when={confirming()} keyed>
              {(d) => (
                <DeployConfirm
                  deploying={d}
                  onClose={() => setConfirming(undefined)}
                  onDone={(text, ok) => {
                    setConfirming(undefined);
                    notify(ok ? t("quality:history.restored") : text, !ok);
                  }}
                />
              )}
            </Show>
          </div>
        </Show>
        <DiffModes />
        <DiffView
          change={{
            file: props.column.file ?? "mapping.yaml",
            before: props.change.before,
            after: props.change.after,
          }}
          mode={diffMode()}
        />
      </details>
    </li>
  );
};

/** The column's open drafts, each to review; then its changes on `main`, newest first. */
const HistoryLists: Component<{ column: Column; onReview: (id: string) => void }> = (props) => {
  const [all] = createResource(drafted, () => draftsOf(props.column.dataset));
  const [changes] = createResource(
    () => ({ ...props.column, n: ruled() }),
    (c) => draftsApi.fileHistory(c.dataset, c.file),
  );
  const [me] = createResource(who);
  // `main`'s file now: the latest change's; only a rules file is deployed.
  const current = () =>
    props.column.file && may(me(), "quality:ApproveRules") && may(me(), "quality:ManageDatasets")
      ? (changes()?.[0]?.after ?? null)
      : undefined;
  return (
    <div class="qd-column-history">
      <h3 class="qd-section">{t("quality:history.drafts")}</h3>
      <ul class="qd-history-list">
        <For
          each={draftsFor(all(), props.column.file)}
          fallback={<li class="qd-hint">{t("quality:drafts.empty")}</li>}
        >
          {(d) => (
            <li>
              <span class="qd-mono">{short(d.id)}</span>{" "}
              <span title={d.author}>{authorName(d.author)}</span>{" "}
              <small class="qd-hint">{doltDate(d.at)}</small>{" "}
              <button type="button" class="fk-btn fk-btn--sm" onClick={() => props.onReview(d.id)}>
                {t("quality:drafts.review")}
              </button>
            </li>
          )}
        </For>
      </ul>
      <h3 class="qd-section">{t("quality:history.main")}</h3>
      <LoadFailed when={changes.error} />
      <ul class="qd-history-list">
        <For each={changes()} fallback={<li class="qd-hint">{t("quality:history.none")}</li>}>
          {(change, i) => (
            <MainChangeItem
              change={change}
              column={props.column}
              current={current()}
              latest={i() === 0}
            />
          )}
        </For>
      </ul>
    </div>
  );
};

/** A column's history over the tree: its lists, or one of its drafts under review. */
const HistoryPanel: Component<{ column: Column; onClose: () => void }> = (props) => {
  const [me] = createResource(who);
  const [reviewing, setReviewing] = createSignal<string>();
  return (
    <Modal
      open
      size="wide"
      title={t("quality:history.title", { column: props.column.label })}
      onClose={props.onClose}
    >
      <div class="qd-modal-scroll">
        <Show
          when={reviewing()}
          fallback={<HistoryLists column={props.column} onReview={setReviewing} />}
        >
          {(id) => (
            <>
              <button
                type="button"
                class="fk-btn fk-btn--sm qd-back"
                onClick={() => setReviewing(undefined)}
              >
                ← {t("quality:history.back")}
              </button>
              <DraftReview
                dataset={props.column.dataset}
                id={id()}
                approves={may(me(), "quality:ApproveRules") && may(me(), "quality:ManageDatasets")}
                discards={may(me(), "quality:ManageDatasets")}
                onDone={(text) => {
                  setReviewing(undefined);
                  notify(text);
                  draftsChanged();
                }}
              />
            </>
          )}
        </Show>
      </div>
    </Modal>
  );
};

/**
 * ⎇ of a column's rules file, in the editor, or of a dataset in the tree
 * (decision 0050), marked while it has an open draft: opens its history,
 * its drafts to review and its changes on `main`. Without a file, the
 * dataset's: its drafts of several files and its mapping.
 */
export const BranchMark: Component<Column & { local?: boolean }> = (props) => {
  // `local`: a button with its name and its own panel, where nothing makes
  // the mark anew (the editor).
  const [mine, setMine] = createSignal(false);
  const [all] = createResource(drafted, () => draftsOf(props.dataset));
  const count = () => draftsFor(all(), props.file).length;
  const title = () =>
    count()
      ? t("quality:history.openDrafts", { column: props.label, count: count() })
      : t("quality:history.title", { column: props.label });
  return (
    <>
      <button
        type="button"
        class={props.local ? "fk-btn qd-history-button" : "qd-tree-branch"}
        classList={{ "qd-tree-branch--open": count() > 0 }}
        title={title()}
        aria-label={title()}
        onClick={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (props.local) setMine(true);
          else setShown({ dataset: props.dataset, file: props.file, label: props.label });
        }}
      >
        <span class="qd-branch-glyph">
          <BranchIcon />
        </span>
        <Show when={props.local}>
          <span>{t("quality:history.button")}</span>
        </Show>
        <Show when={count() > 0}>
          <span class="qd-tree-branch__count">{count()}</span>
        </Show>
      </button>
      <Show when={mine()}>
        <HistoryPanel
          column={{ dataset: props.dataset, file: props.file, label: props.label }}
          onClose={() => setMine(false)}
        />
      </Show>
    </>
  );
};

/**
 * The column whose history is open. Kept here, not in its ⎇: a deploy asks
 * the tree's rules again and its columns are made anew, the panel with them.
 */
const [shown, setShown] = createSignal<Column>();

/** The open column's history, once, where the tree's changes do not reach it. */
export const ColumnHistoryHost: Component = () => (
  <Show when={shown()} keyed>
    {(column) => <HistoryPanel column={column} onClose={() => setShown(undefined)} />}
  </Show>
);
