import "./settings.css";
import { createResource, createSignal, Show, type Component } from "solid-js";
import { formatDateTime } from "@knime-orchestador/i18n";
import { SettingsApiError } from "./api";
import { Field, NoticeBox, Panel, SecretInput, useAction } from "./common";
import { t } from "./i18n";
import {
  ADMIN_ACCESS_KEY,
  keptState,
  loadAdminKeys,
  saveAdminKeys,
  type LakeKeys,
} from "./lakeAdmin";
import { mainLine } from "./lakeMain";
import { replicaLine, type ReplicaLine } from "./lakeReplica";
import { LakePoolCard } from "./LakePoolCard";
import { LakeReplicationCard } from "./LakeReplicationCard";
import { machinesApi, type MachinesApi } from "./machinesApi";

/** Whether a lake's admin keys are kept, and since when. */
const KeptLine: Component<{ state: ReturnType<typeof keptState>; none: string }> = (props) => (
  <Show
    when={props.state.kind === "kept" && props.state}
    fallback={<p class="st-notice st-notice--warn">{props.none}</p>}
  >
    {(kept) => (
      <p class="table__muted">
        {t("settings:lake.admin.kept", {
          access: kept().accessKey,
          at: formatDateTime(kept().setAt),
        })}
      </p>
    )}
  </Show>
);

/** What a lake answered, one line, asked again on demand. */
const StateCard: Component<{
  texts: "main" | "replica";
  asked: number;
  load: () => Promise<ReplicaLine>;
}> = (props) => {
  const [state, { refetch }] = createResource(() => props.asked, props.load);
  return (
    <div class="card">
      <h2 class="st-subtitle">{t(`settings:lake.${props.texts}.title`)}</h2>
      <p>{t(`settings:lake.${props.texts}.intro`)}</p>
      <Show when={state.error}>
        <p class="st-notice st-notice--error">
          {state.error instanceof SettingsApiError
            ? state.error.message
            : t(`settings:lake.${props.texts}.loadError`)}
        </p>
      </Show>
      <Show when={state.latest}>
        {(line) => {
          // The plain notice reads as success; the others have their own.
          const tone = () => (line().tone === "ok" ? "" : ` st-notice--${line().tone}`);
          return <p class={`st-notice${tone()}`}>{line().text}</p>;
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

/**
 * A lake's platform admin keys, shown once on its server and pasted here to
 * be kept sealed. The secret is in this form's state alone, never logged,
 * stored or put in a URL, and cleared once the API keeps it.
 */
const AdminKeysCard: Component<{ api: MachinesApi; lake: LakeKeys; onKept: () => void }> = (
  props,
) => {
  const texts = props.lake === "main" ? "mainAdmin" : "admin";
  const [kept, { mutate }] = createResource(() => loadAdminKeys(props.api, props.lake));
  const [access, setAccess] = createSignal(ADMIN_ACCESS_KEY);
  const [secret, setSecret] = createSignal("");
  const action = useAction();

  const submit = (e: Event) => {
    e.preventDefault();
    void action.run(async () => {
      const saved = await saveAdminKeys(props.api, props.lake, access(), secret());
      setSecret("");
      mutate(saved);
      props.onKept();
      return t(`settings:lake.${texts}.saved`);
    });
  };

  return (
    <div class="card">
      <h2 class="st-subtitle">{t(`settings:lake.${texts}.title`)}</h2>
      <p>{t(`settings:lake.${texts}.intro`)}</p>
      <Show when={kept.error}>
        <p class="st-notice st-notice--error">
          {kept.error instanceof SettingsApiError
            ? kept.error.message
            : t("settings:lake.admin.loadError")}
        </p>
      </Show>
      <Show when={kept.latest !== undefined}>
        <KeptLine state={keptState(kept.latest)} none={t(`settings:lake.${texts}.none`)} />
      </Show>
      <form class="st-form" onSubmit={submit}>
        <Field
          label={t("settings:lake.admin.accessKey")}
          error={action.fields().access_key}
          hint={t(`settings:lake.${texts}.accessKeyHint`)}
        >
          <input
            value={access()}
            spellcheck={false}
            autocomplete="off"
            required
            onInput={(e) => setAccess(e.currentTarget.value)}
          />
        </Field>
        <Field
          label={t("settings:lake.admin.secret")}
          error={action.fields().secret_key}
          hint={t(`settings:lake.${texts}.secretHint`)}
        >
          <SecretInput
            value={secret()}
            autocomplete="off"
            required
            placeholder={t("settings:lake.admin.secretPlaceholder")}
            onInput={setSecret}
          />
        </Field>
        <NoticeBox notice={action.notice()} />
        <div class="st-actions">
          <button type="submit" class="fk-btn st-primary" disabled={action.busy()}>
            {action.busy() ? t("settings:lake.admin.saving") : t("settings:lake.admin.save")}
          </button>
        </div>
      </form>
    </div>
  );
};

/**
 * Settings → Lake (decisions 0028, 0029 and 0066 step 11): the main lake,
 * its pool and each pool server's command, the replica, whether each answers with the kept keys, their platform
 * admin keys, which `infra/rustfs/admin-user.sh` and `join.sh` showed once
 * on their servers, and the replication the application configures with
 * them.
 */
export const LakePage: Component<{ api?: MachinesApi }> = (props) => {
  const api = () => props.api ?? machinesApi;
  // Bumped once keys are kept, so the lake is asked again with them.
  const [mainAsked, setMainAsked] = createSignal(0);
  const [replicaAsked, setReplicaAsked] = createSignal(0);

  return (
    <Panel title={t("settings:lake.title")} subtitle={t("settings:lake.subtitle")}>
      <StateCard
        texts="main"
        asked={mainAsked()}
        load={async () => mainLine(await api().mainState())}
      />
      <AdminKeysCard api={api()} lake="main" onKept={() => setMainAsked((n) => n + 1)} />
      <LakePoolCard api={api()} />
      <StateCard
        texts="replica"
        asked={replicaAsked()}
        load={async () => replicaLine(await api().replicaState())}
      />
      <AdminKeysCard api={api()} lake="replica" onKept={() => setReplicaAsked((n) => n + 1)} />
      <LakeReplicationCard api={api()} asked={mainAsked() + replicaAsked()} />
    </Panel>
  );
};
