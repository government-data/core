import "./quality.css";
import { createEffect, createResource, createSignal, on, Show, type Component } from "solid-js";
import { A, useParams, useSearchParams } from "@solidjs/router";
import { draftsApi, may, me, rulesApi } from "./api";
import { createChecker } from "./checker";
import { ruleLine } from "./editor";
import { createTidy } from "./tidy";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { BranchMark } from "./ColumnHistory";
import { ruled } from "./explorerCache";
import { DraftPicker, FileTabs } from "./FileTabs";
import { emptyFile, fieldFile, withNewRule } from "./newRule";
import { keyOf, openFiles, type OpenFile } from "./openFiles";
import { SaveBar } from "./SaveBar";
import { Workspace } from "./Workspace";

/** The page's title: back to the rules, the dataset and file, whose they are. */
const Title: Component<{ dataset: string; file: string; draft?: string }> = (props) => (
  <div>
    <A class="qd-link" href={`/quality/rules?dataset=${encodeURIComponent(props.dataset)}`}>
      ← {t("quality:editor.back")}
    </A>
    <h1>
      {props.dataset} <span class="qd-mono qd-hint">{props.file}</span>
    </h1>
    <p class="qd-hint">
      {props.draft ? t("quality:editor.onDraft") : t("quality:editor.subtitle")}
    </p>
  </div>
);

/** Whether the text differs from the file as saved, and going back to it. */
const Unsaved: Component<{ changed: boolean; onRestore: () => void }> = (props) => (
  <Show when={props.changed}>
    <span class="qd-hint">{t("quality:editor.changed")}</span>
    <button type="button" class="fk-btn" onClick={() => props.onRestore()}>
      {t("quality:editor.restore")}
    </button>
  </Show>
);

/** A rules file as `main` or a draft has it. */
type RulesFile = { file: string; yaml: string };

/** The rules files of `dataset` on `main`, or on its draft `draft`. */
async function files(key: { dataset: string; draft?: string }) {
  if (!key.draft) return (await rulesApi.read(key.dataset)).files;
  return (await draftsApi.read(key.dataset, key.draft)).rules;
}

/** The editor's head: the file, main or a draft, what is unsaved and _Save_. */
const EditorHead: Component<{
  dataset: string;
  file: string;
  draft?: string;
  onPick: (draft: string | undefined) => void;
  changed: boolean;
  onRestore: () => void;
  /** May edit the rules, and the file is there. */
  edits: boolean;
  /** May deploy to `main` (decision 0051). */
  deploys: boolean;
  /** `main`'s file as opened; `null` if it has none. */
  expected: string | null;
  onDeployed: () => void;
  text: string;
  blocked: boolean;
  onSaved: (draft: string) => void;
  onRefused: Parameters<typeof SaveBar>[0]["onRefused"];
}> = (props) => (
  <header class="qd-head">
    <Title dataset={props.dataset} file={props.file} draft={props.draft} />
    <div class="qd-actions">
      <DraftPicker
        dataset={props.dataset}
        file={props.file}
        draft={props.draft}
        onPick={props.onPick}
      />
      <Unsaved changed={props.changed} onRestore={props.onRestore} />
      <Show when={props.edits}>
        <SaveBar
          dataset={props.dataset}
          file={props.file}
          draft={props.draft}
          deploys={props.deploys}
          expected={props.expected}
          text={props.text}
          changed={props.changed}
          blocked={props.blocked}
          onSaved={props.onSaved}
          onDeployed={props.onDeployed}
          onRefused={props.onRefused}
        />
      </Show>
      <BranchMark dataset={props.dataset} file={props.file} label={props.file} local />
    </div>
  </header>
);

/**
 * A file's text as saved: as `main` or the draft has it, or, for a
 * field's own file they lack, empty: saving adds it (decision 0044).
 * `undefined` while loading, or for any other file not there.
 */
function textOf(files: RulesFile[] | undefined, dataset: string, file: string) {
  if (!files) return undefined;
  const yaml = files.find((f) => f.file === file)?.yaml;
  if (yaml !== undefined) return yaml;
  return fieldFile(file.replace(/\.yaml$/, "")) ? emptyFile(dataset, file) : undefined;
}

/** Under the head: the file is not there, or is a new one. */
const FileNotice: Component<{ files?: RulesFile[]; file: string; saved?: string }> = (props) => (
  <Show when={props.files && !props.files.some((f) => f.file === props.file)}>
    <Show
      when={props.saved !== undefined}
      fallback={<p class="qd-error">{t("quality:editor.noFile")}</p>}
    >
      <p class="qd-hint">{t("quality:editor.newFile")}</p>
    </Show>
  </Show>
);

/**
 * _+_ in the tree (`?add=`): once the file is loaded, a rule for the field
 * at the end of it as it is, the editor at its line, unsaved.
 */
function addWhenAsked(
  field: () => string | undefined,
  saved: () => string | undefined,
  add: (field: string, yaml: string) => void,
) {
  createEffect(
    on([field, saved], ([f, yaml]) => {
      if (f && yaml !== undefined) add(f, yaml);
    }),
  );
}

/**
 * What follows a save: nothing unsaved; a first save moves the tab onto
 * its new draft; a deploy (decision 0051) reads `main` again.
 */
function afterSaving(
  current: () => OpenFile,
  draft: () => string | undefined,
  setSearch: (s: { draft: string }, o: { replace: boolean }) => void,
  refetch: () => unknown,
) {
  const onSaved = (id: string) => {
    openFiles.type(current(), undefined);
    if (draft() === id) return void refetch();
    openFiles.close(current());
    setSearch({ draft: id }, { replace: true });
  };
  const onDeployed = () => {
    openFiles.type(current(), undefined);
    void refetch();
  };
  return { onSaved, onDeployed };
}

/**
 * _Rule_ (decision 0019): a rules file in YAML, of `main` or of a draft
 * (`?draft=`), checked as it is typed (lint with its `QR` codes on the
 * lines, and each rule as read beside it), opened at the rule asked for
 * (`?rule=`) or with a rule added for a field (`?add=`); deployed to
 * `main` by whoever may approve the rules (0051), else saved as a draft.
 */
export const RuleEditorPage: Component = () => {
  const params = useParams<{ dataset: string; file: string }>();
  const [search, setSearch] = useSearchParams<{ rule?: string; draft?: string; add?: string }>();
  const [source, { refetch }] = createResource(
    // Asked again when a version is restored from its history (0051).
    () => ({ dataset: params.dataset, draft: search.draft, n: ruled() }),
    files,
  );
  const [who] = createResource(me);
  const [schema] = createResource(rulesApi.schema);
  const saved = () => textOf(source(), params.dataset, params.file);
  const [text, setText] = createSignal<string>();
  // What the editor must show; set again on restore, even if equal.
  const [shown, setShown] = createSignal("", { equals: false });
  // Each go counts, even to the line asked last.
  const [line, setLine] = createSignal<number | undefined>(undefined, { equals: false });
  const checker = createChecker(() => ({ ...params, draft: search.draft }));
  const changed = () => text() !== undefined && text() !== saved();

  // The tab: this file, of `main` or of the draft.
  const current = (): OpenFile => ({ ...params, draft: search.draft || undefined });
  createEffect(() => openFiles.open(current()));

  // The file as saved, or as typed when its tab was left: shown, checked,
  // opened at the rule asked for the first time the tab shows.
  let showing: string | undefined;
  createEffect(
    on([saved, () => keyOf(current())], ([yaml, key]) => {
      if (yaml === undefined) return;
      const fresh = key !== showing;
      showing = key;
      if (fresh) setLine(search.rule ? ruleLine(yaml, search.rule) : undefined);
      const yamlShown = (fresh && openFiles.unsaved(current())) || yaml;
      setText(yamlShown);
      setShown(yamlShown);
      void checker.now(yamlShown);
    }),
  );

  const typed = (yaml: string) => {
    setText(yaml);
    openFiles.type(current(), yaml === saved() ? undefined : yaml);
    checker.typed(yaml);
  };

  addWhenAsked(
    () => search.add,
    saved,
    (field, yaml) => {
      const added = withNewRule(text() ?? yaml, field);
      setSearch({ add: undefined }, { replace: true });
      setShown(added.yaml);
      typed(added.yaml);
      setLine(added.line);
    },
  );

  const restore = () => {
    const yaml = saved();
    if (yaml === undefined) return;
    openFiles.type(current(), undefined);
    setShown(yaml);
    setText(yaml);
    void checker.now(yaml);
  };

  const tidy = createTidy(text, setShown, checker.check);

  const { onSaved, onDeployed } = afterSaving(current, () => search.draft, setSearch, refetch);

  return (
    <section class="feature qd-page">
      <FileTabs current={current()} />
      <EditorHead
        dataset={params.dataset}
        file={params.file}
        draft={search.draft}
        onPick={(draft) => setSearch({ draft, rule: undefined })}
        changed={changed()}
        onRestore={restore}
        edits={may(who(), "quality:EditRules") && saved() !== undefined}
        deploys={may(who(), "quality:ApproveRules") && may(who(), "quality:ManageDatasets")}
        expected={source()?.find((f) => f.file === params.file)?.yaml ?? null}
        onDeployed={onDeployed}
        text={text() ?? ""}
        blocked={checker.blocked()}
        onSaved={onSaved}
        onRefused={checker.refused}
      />
      <LoadFailed when={source.error} />
      <FileNotice files={source()} file={params.file} saved={saved()} />
      <Show when={saved() !== undefined}>
        <Workspace
          dataset={params.dataset}
          file={params.file}
          shown={shown()}
          text={text() ?? ""}
          line={line()}
          schema={schema()}
          checker={checker}
          tidy={tidy}
          onChange={typed}
          onGo={setLine}
        />
      </Show>
    </section>
  );
};
