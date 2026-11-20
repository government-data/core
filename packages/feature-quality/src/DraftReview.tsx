import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { A } from "@solidjs/router";
import { ConfirmDialog, Status } from "@knime-orchestador/feature-kit/ui";
import { draftsApi, QualityError, type DraftOut } from "./api";
import { approvable, approvalRefusal, errorCount } from "./drafts";
import { DraftHistory } from "./DraftHistory";
import { t } from "./i18n";
import { LoadFailed } from "./Notify";
import { Tabs } from "./Tabs";

/** A file of the draft, its status and its YAML folded. */
const DraftFile: Component<{ name: string; yaml: string; errors?: string[]; edit?: string }> = (
  props,
) => (
  <details class="qd-file">
    <summary>
      <span class="qd-mono">{props.name}</span>
      <Show when={props.edit}>
        {(href) => (
          <A class="qd-link" href={href()}>
            {t("quality:common.edit")}
          </A>
        )}
      </Show>
      <Show when={props.errors}>
        {(errors) => (
          <Show
            when={errors().length > 0}
            fallback={<Status tone="good" label={t("quality:drafts.noErrors")} />}
          >
            <Status
              tone="critical"
              label={t("quality:drafts.errors", { count: errors().length })}
            />
            <span class="qd-mono qd-hint">{errors().join(", ")}</span>
          </Show>
        )}
      </Show>
    </summary>
    <pre class="qd-yaml">{props.yaml}</pre>
  </details>
);

/** The draft's mapping, whether it reads, and its rules files. */
const DraftFiles: Component<{ draft: DraftOut; dataset: string; id: string }> = (props) => (
  <>
    <h3 class="qd-section">
      {t("quality:drafts.mapping")}{" "}
      <Status
        tone={props.draft.mapping_reads ? "good" : "critical"}
        label={t(`quality:drafts.${props.draft.mapping_reads ? "mappingReads" : "mappingBroken"}`)}
      />
    </h3>
    <DraftFile name="mapping.yaml" yaml={props.draft.mapping} />
    <h3 class="qd-section">
      {t("quality:drafts.rules")}{" "}
      <span class="qd-hint">{t("quality:drafts.errors", { count: errorCount(props.draft) })}</span>
    </h3>
    <For each={props.draft.rules}>
      {(file) => (
        <DraftFile
          name={file.file}
          yaml={file.yaml}
          errors={file.errors}
          edit={`/quality/rules/${encodeURIComponent(props.dataset)}/${encodeURIComponent(file.file)}?draft=${encodeURIComponent(props.id)}`}
        />
      )}
    </For>
  </>
);

/** The draft's changes as a branch (by default) or its files as they are. */
const DraftViews: Component<{ draft: DraftOut; dataset: string; id: string }> = (props) => {
  const [view, setView] = createSignal<"changes" | "files">("changes");
  return (
    <>
      <Tabs
        tabs={[
          { id: "changes", label: t("quality:drafts.history.tabChanges") },
          { id: "files", label: t("quality:drafts.history.tabFiles") },
        ]}
        current={view()}
        onChoose={(id) => setView(id as "changes" | "files")}
      />
      <Show
        when={view() === "changes"}
        fallback={<DraftFiles draft={props.draft} dataset={props.dataset} id={props.id} />}
      >
        <DraftHistory dataset={props.dataset} id={props.id} />
      </Show>
    </>
  );
};

/**
 * A draft to review (decisions 0021 and 0038): its mapping, whether it
 * reads, and each rules file with its lint errors; approved into `main`
 * only when nothing is wrong, or discarded.
 */
export const DraftReview: Component<{
  dataset: string;
  id: string;
  approves: boolean;
  discards: boolean;
  onDone: (text: string) => void;
}> = (props) => {
  const [draft, { mutate }] = createResource(
    () => ({ dataset: props.dataset, id: props.id }),
    (key) => draftsApi.read(key.dataset, key.id),
  );
  const [refusal, setRefusal] = createSignal<string>();
  const [discarding, setDiscarding] = createSignal(false);
  const [pending, setPending] = createSignal(false);

  const approve = async () => {
    setRefusal(undefined);
    setPending(true);
    try {
      await draftsApi.approve(props.dataset, props.id);
      props.onDone(t("quality:drafts.approved", { dataset: props.dataset }));
    } catch (err) {
      const status = err instanceof QualityError ? err.status : 0;
      const body = err instanceof QualityError ? err.body : undefined;
      if (status === 422 && (body as DraftOut | undefined)?.rules) mutate(body as DraftOut);
      setRefusal(t(`quality:drafts.refusal.${approvalRefusal(status, body)}`));
    } finally {
      setPending(false);
    }
  };

  const discard = async () => {
    setPending(true);
    try {
      await draftsApi.discard(props.dataset, props.id);
      props.onDone(t("quality:drafts.discarded"));
    } catch (err) {
      const status = err instanceof QualityError ? err.status : 0;
      setRefusal(t(`quality:drafts.refusal.${approvalRefusal(status, undefined)}`));
    } finally {
      setPending(false);
      setDiscarding(false);
    }
  };

  return (
    <div class="card">
      <LoadFailed when={draft.error} />
      <Show when={draft()}>
        {(d) => (
          <>
            <div class="qd-card-head">
              <h2>
                {props.dataset} <span class="qd-mono qd-hint">{props.id}</span>
              </h2>
              <div class="qd-actions">
                <Show when={props.discards}>
                  <button
                    class="fk-btn fk-btn--danger"
                    disabled={pending()}
                    onClick={() => setDiscarding(true)}
                  >
                    {t("quality:drafts.discard")}
                  </button>
                </Show>
                <Show when={props.approves}>
                  <button
                    class="fk-btn fk-btn--primary"
                    disabled={pending() || !approvable(d())}
                    onClick={() => void approve()}
                  >
                    {t("quality:drafts.approve")}
                  </button>
                </Show>
              </div>
            </div>
            <Show when={refusal()}>{(r) => <p class="qd-error">{r()}</p>}</Show>
            <Show when={!approvable(d())}>
              <p class="qd-hint">{t("quality:drafts.blocked")}</p>
            </Show>
            <DraftViews draft={d()} dataset={props.dataset} id={props.id} />
          </>
        )}
      </Show>
      <ConfirmDialog
        open={discarding()}
        title={t("quality:drafts.discard")}
        message={t("quality:drafts.confirmDiscard")}
        pending={pending()}
        onCancel={() => setDiscarding(false)}
        onConfirm={() => void discard()}
      />
    </div>
  );
};
