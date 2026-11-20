/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { settingsApi, type LdapProbe, type LdapSettings, type Source } from "./api";
import {
  Field,
  lines,
  NoticeBox,
  SecretInput,
  SectionHeader,
  SourceNote,
  Toggle,
  useAction,
} from "./common";
import { t } from "./i18n";

const DEFAULT_FILTER = "(&(objectClass=user)(|(userPrincipalName={login})(mail={login})))";

const EMPTY: LdapSettings = {
  enabled: false,
  url: "",
  starttls: false,
  tls_insecure: false,
  bind_dn: "",
  bind_password: "",
  base_dn: "",
  user_filter: DEFAULT_FILTER,
  admin_groups: [],
  timeout_secs: 5,
};

export const LdapPage: Component = () => {
  const [form, setForm] = createStore<LdapSettings>({ ...EMPTY });
  const [source, setSource] = createSignal<Source>();
  const [groupsText, setGroupsText] = createSignal("");
  const [testLogin, setTestLogin] = createSignal("");
  const [probe, setProbe] = createSignal<LdapProbe>();
  const save = useAction();
  const test = useAction();

  const load = (value: LdapSettings, src: Source) => {
    setForm(reconcile({ ...EMPTY, ...value, user_filter: value.user_filter || DEFAULT_FILTER }));
    setGroupsText(value.admin_groups.join("\n"));
    setSource(src);
  };

  const [settings] = createResource(async () => {
    const all = await settingsApi.all();
    load(all.ldap.value, all.ldap.source);
    return all;
  });

  const payload = (): LdapSettings => ({ ...form, admin_groups: lines(groupsText()) });
  const err = (f: string) => save.fields()[f] ?? test.fields()[f];

  const isAdmin = (groups: string[]) => {
    const admin = lines(groupsText()).map((g) => g.toLowerCase());
    return groups.some((g) => admin.includes(g.toLowerCase()));
  };

  return (
    <section class="feature">
      <SectionHeader
        title={t("settings:ldap.ldapActiveDirectory")}
        subtitle={t("settings:ldap.intro")}
      />
      <Show when={settings.error}>
        <p class="st-notice st-notice--error">{t("settings:ldap.readError")}</p>
      </Show>
      <Show when={source()}>
        {(src) => (
          <SourceNote
            source={src()}
            onReset={() =>
              save.run(async () => {
                const all = await settingsApi.reset("ldap");
                load(all.ldap.value, all.ldap.source);
                return t("settings:ldap.switchedToEnv");
              })
            }
          />
        )}
      </Show>

      <form
        class="card st-form"
        onSubmit={(e) => {
          e.preventDefault();
          void save.run(async () => {
            const saved = await settingsApi.saveLdap(payload());
            load(saved.value, saved.source);
            return saved.value.enabled ? t("settings:ldap.savedOn") : t("settings:ldap.savedOff");
          });
        }}
      >
        <Toggle
          checked={form.enabled}
          onChange={(v) => setForm("enabled", v)}
          tip={t("settings:ldap.offHelp")}
        >
          {t("settings:ldap.enable")}
        </Toggle>

        <fieldset class="st-group" disabled={!form.enabled}>
          <legend>{t("settings:ldap.connection")}</legend>
          <Field
            label={t("settings:ldap.serverUrl")}
            error={err("url")}
            tip={t("settings:ldap.serverUrlHelp")}
            hint={t("settings:ldap.serverUrlPlaceholder")}
          >
            <input
              value={form.url}
              placeholder="ldaps://dc01.empresa.local:636"
              onInput={(e) => setForm("url", e.currentTarget.value)}
            />
          </Field>
          <div class="st-row">
            <Toggle
              checked={form.starttls}
              onChange={(v) => setForm("starttls", v)}
              tip={t("settings:ldap.starttlsHelp")}
            >
              {t("settings:ldap.starttlsLdapOnly")}
            </Toggle>
            <Toggle
              checked={form.tls_insecure}
              onChange={(v) => setForm("tls_insecure", v)}
              tip={t("settings:ldap.insecureHelp")}
            >
              {t("settings:ldap.insecure")}
            </Toggle>
          </div>
          <Show when={err("starttls")}>
            <span class="st-field__error">{err("starttls")}</span>
          </Show>
          <Field
            label={t("settings:ldap.timeoutSeconds")}
            error={err("timeout_secs")}
            tip={t("settings:ldap.timeoutHelp")}
            hint={t("settings:ldap.timeoutNote")}
          >
            <input
              type="number"
              min="1"
              max="120"
              value={form.timeout_secs}
              onInput={(e) => setForm("timeout_secs", Number(e.currentTarget.value) || 0)}
            />
          </Field>
        </fieldset>

        <fieldset class="st-group" disabled={!form.enabled}>
          <legend>{t("settings:ldap.serviceAccount")}</legend>
          <p class="st-help">{t("settings:ldap.serviceAccountHelp")}</p>
          <Field
            label={t("settings:ldap.accountDn")}
            error={err("bind_dn")}
            tip={
              <>
                {t("settings:ldap.bindDnHelp")} <code>distinguishedName</code>
                {t("settings:ldap.orWith")}
                <code>{t("settings:ldap.getAduserSvcKnime")}</code>
                {t("settings:ldap.regularUserEnough")}
              </>
            }
          >
            <input
              value={form.bind_dn}
              placeholder="CN=svc-knime,OU=Servicios,DC=empresa,DC=local"
              onInput={(e) => setForm("bind_dn", e.currentTarget.value)}
            />
          </Field>
          <Field
            label={t("settings:ldap.password")}
            error={err("bind_password")}
            tip={t("settings:ldap.passwordHelp")}
          >
            <SecretInput
              value={form.bind_password}
              saved={form.has_bind_password}
              onInput={(v) => setForm("bind_password", v)}
            />
          </Field>
        </fieldset>

        <fieldset class="st-group" disabled={!form.enabled}>
          <legend>{t("settings:ldap.usersGroups")}</legend>
          <Field
            label={t("settings:ldap.searchBaseDn")}
            error={err("base_dn")}
            tip={t("settings:ldap.searchBaseHelp")}
          >
            <input
              value={form.base_dn}
              placeholder="DC=empresa,DC=local"
              onInput={(e) => setForm("base_dn", e.currentTarget.value)}
            />
          </Field>
          <Field
            label={t("settings:ldap.userFilter")}
            error={err("user_filter")}
            tip={
              <>
                {t("settings:ldap.userFilterHelp")}{" "}
                <code>(memberOf=CN=Orquestador-Usuarios,OU=Grupos,DC=…)</code>
                {t("settings:ldap.tryWithTest")}
              </>
            }
            hint={
              <>
                <code>{"{login}"}</code> {t("settings:ldap.placeholderNote")}
              </>
            }
          >
            <textarea
              rows="2"
              value={form.user_filter}
              onInput={(e) => setForm("user_filter", e.currentTarget.value)}
            />
          </Field>
          <Field
            label={t("settings:ldap.administratorGroups")}
            tip={t("settings:ldap.adminGroupsHelp")}
            hint={t("settings:ldap.adminGroupsNote")}
          >
            <textarea
              rows="3"
              value={groupsText()}
              placeholder="CN=Orquestador-Admins,OU=Grupos,DC=empresa,DC=local"
              onInput={(e) => setGroupsText(e.currentTarget.value)}
            />
          </Field>
        </fieldset>

        <NoticeBox notice={save.notice()} />
        <div class="st-actions">
          <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
            {save.busy() ? t("settings:ldap.saving") : t("settings:ldap.save")}
          </button>
        </div>
      </form>

      <div class="card st-form">
        <h2 class="st-subtitle">{t("settings:ldap.testConnection")}</h2>
        <p class="st-help">{t("settings:ldap.usesForm")}</p>
        <div class="st-inline">
          <input
            value={testLogin()}
            placeholder={t("settings:ldap.testUserPlaceholder")}
            onInput={(e) => setTestLogin(e.currentTarget.value)}
          />
          <button
            type="button"
            class="fk-btn"
            disabled={test.busy()}
            onClick={() =>
              test.run(async () => {
                setProbe(undefined);
                const result = await settingsApi.testLdap(
                  { ...payload(), enabled: true },
                  testLogin(),
                );
                setProbe(result);
                if (!result.searched) return t("settings:ldap.bindOk");
                return result.user ? t("settings:ldap.userFound") : t("settings:ldap.userNotFound");
              })
            }
          >
            {test.busy() ? t("settings:ldap.testing") : t("settings:ldap.test")}
          </button>
        </div>
        <NoticeBox notice={test.notice()} />
        <Show when={probe()?.user}>
          {(u) => (
            <dl class="st-probe">
              <dt>DN</dt>
              <dd>
                <code>{u().dn}</code>
              </dd>
              <dt>{t("settings:ldap.email")}</dt>
              <dd>{u().email ?? t("settings:ldap.noMail")}</dd>
              <dt>{t("settings:ldap.name")}</dt>
              <dd>{u().display_name ?? "—"}</dd>
              <dt>{t("settings:ldap.administrator")}</dt>
              <dd>{isAdmin(u().groups) ? t("settings:ldap.yes") : t("settings:ldap.noPerArea")}</dd>
              <dt>{t("settings:ldap.directGroups")}</dt>
              <dd>
                <Show when={u().groups.length > 0} fallback="Ninguno">
                  <ul class="st-list">
                    <For each={u().groups}>
                      {(g) => (
                        <li>
                          <code>{g}</code>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </dd>
            </dl>
          )}
        </Show>
      </div>
    </section>
  );
};
