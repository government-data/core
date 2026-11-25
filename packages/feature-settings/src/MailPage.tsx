/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, Show, type Component } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { useSession } from "@knime-orchestador/feature-kit";
import { settingsApi, type MailSettings, type SmtpSecurity, type Source } from "./api";
import {
  Field,
  NoticeBox,
  SecretInput,
  SectionHeader,
  SourceNote,
  Toggle,
  useAction,
} from "./common";
import { t, translated } from "./i18n";

const EMPTY: MailSettings = translated(() => ({
  enabled: false,
  transport: "graph",
  from_address: "",
  from_name: t("settings:mail.defaultDisplayName"),
  smtp_host: "",
  smtp_port: 587,
  smtp_security: "starttls",
  smtp_username: "",
  smtp_password: "",
  graph_tenant_id: "",
  graph_client_id: "",
  graph_client_secret: "",
  reset_link_minutes: 0,
  notice_language: "es",
}));

const DEFAULT_PORT: Record<SmtpSecurity, number> = { none: 25, starttls: 587, tls: 465 };

export const MailPage: Component = () => {
  const session = useSession();
  const [form, setForm] = createStore<MailSettings>({ ...EMPTY });
  const [source, setSource] = createSignal<Source>();
  const [testTo, setTestTo] = createSignal(session.email ?? "");
  const save = useAction();
  const test = useAction();

  const load = (value: MailSettings, src: Source) => {
    // What was never saved starts with Office 365 chosen.
    setForm(reconcile(src.kind === "unset" ? { ...EMPTY } : { ...EMPTY, ...value }));
    setSource(src);
  };

  const [settings] = createResource(async () => {
    const all = await settingsApi.all();
    load(all.mail.value, all.mail.source);
    return all;
  });

  const err = (f: string) => save.fields()[f] ?? test.fields()[f];

  return (
    <section class="feature">
      <SectionHeader title={t("settings:mail.outgoingMail")} subtitle={t("settings:mail.intro")} />
      <Show when={settings.error}>
        <p class="st-notice st-notice--error">{t("settings:mail.readError")}</p>
      </Show>
      <Show when={settings()}>
        <Show when={!form.enabled}>
          <p class="st-notice st-notice--warn" role="status">
            <strong>{t("settings:mail.required")}</strong> {t("settings:mail.noOwnServer")}
          </p>
        </Show>
        <Show when={source()}>
          {(src) => (
            <SourceNote
              source={src()}
              onReset={() =>
                save.run(async () => {
                  const all = await settingsApi.reset("mail");
                  load(all.mail.value, all.mail.source);
                  return t("settings:mail.deleted");
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
              const saved = await settingsApi.saveMail({ ...form });
              load(saved.value, saved.source);
              return saved.value.enabled ? t("settings:mail.savedOn") : t("settings:mail.saved");
            });
          }}
        >
          <Toggle
            checked={form.enabled}
            onChange={(v) => setForm("enabled", v)}
            tip={t("settings:mail.offHelp")}
          >
            {t("settings:mail.sendMail")}
          </Toggle>

          <div class="st-segmented" role="radiogroup" aria-label={t("settings:mail.mailProvider")}>
            <button
              type="button"
              role="radio"
              aria-checked={form.transport === "graph"}
              classList={{ "st-segmented--on": form.transport === "graph" }}
              onClick={() => setForm("transport", "graph")}
            >
              {t("settings:mail.office365MicrosoftGraph")}
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={form.transport === "smtp"}
              classList={{ "st-segmented--on": form.transport === "smtp" }}
              onClick={() => setForm("transport", "smtp")}
            >
              {t("settings:mail.smtpServer")}
            </button>
          </div>

          <fieldset class="st-group">
            <legend>{t("settings:mail.sender")}</legend>
            <div class="st-row">
              <Field
                label={
                  form.transport === "graph"
                    ? t("settings:mail.senderMailbox")
                    : t("settings:mail.senderAddress")
                }
                error={err("from_address")}
                tip={
                  form.transport === "graph"
                    ? t("settings:mail.senderMailboxHelp")
                    : t("settings:mail.senderAddressHelp")
                }
                hint={form.transport === "graph" ? t("settings:mail.senderMailboxNote") : undefined}
              >
                <input
                  type="email"
                  value={form.from_address}
                  placeholder="knime@empresa.com"
                  onInput={(e) => setForm("from_address", e.currentTarget.value)}
                />
              </Field>
              <Field
                label={t("settings:mail.displayName")}
                tip={t("settings:mail.displayNameHelp")}
              >
                <input
                  value={form.from_name}
                  onInput={(e) => setForm("from_name", e.currentTarget.value)}
                />
              </Field>
            </div>
          </fieldset>

          <Show when={form.transport === "graph"}>
            <fieldset class="st-group">
              <legend>{t("settings:mail.appRegistration")}</legend>
              <details class="st-howto">
                <summary>{t("settings:mail.howCreate")}</summary>
                <ol>
                  <li>
                    {t("settings:mail.in")}{" "}
                    <strong>{t("settings:mail.appRegistrationsPath")}</strong>
                    {t("settings:mail.createApp")}
                  </li>
                  <li>
                    {t("settings:mail.in")} <strong>{t("settings:mail.apiPermissions")}</strong>
                    {t("settings:mail.addGraph")}
                    <em>{t("settings:mail.applicationPermissions")}</em> → <code>Mail.Send</code>{" "}
                    {t("settings:mail.andClick")}
                    <strong>{t("settings:mail.grantAdminConsent")}</strong>.
                  </li>
                  <li>
                    {t("settings:mail.in")}{" "}
                    <strong>{t("settings:mail.certificatesSecrets")}</strong>
                    {t("settings:mail.createSecret")}
                    <em>{t("settings:mail.value")}</em> {t("settings:mail.notTheId")}
                  </li>
                  <li>
                    {t("settings:mail.restrictRecommended")}
                    <em>{t("settings:mail.applicationAccessPolicy")}</em>{" "}
                    {t("settings:mail.inExchangeOnline")}
                    <code>New-ApplicationAccessPolicy</code>
                    {t("settings:mail.withoutRestriction")}
                  </li>
                </ol>
              </details>
              <Field
                label={t("settings:mail.tenantId")}
                tip={t("settings:mail.tenantIdHelp")}
                error={err("graph_tenant_id")}
                hint={t("settings:mail.tenantIdPlaceholder")}
              >
                <input
                  value={form.graph_tenant_id}
                  placeholder="00000000-0000-0000-0000-000000000000"
                  onInput={(e) => setForm("graph_tenant_id", e.currentTarget.value)}
                />
              </Field>
              <Field
                label={t("settings:mail.clientId")}
                error={err("graph_client_id")}
                tip={t("settings:mail.clientIdHelp")}
              >
                <input
                  value={form.graph_client_id}
                  placeholder="00000000-0000-0000-0000-000000000000"
                  onInput={(e) => setForm("graph_client_id", e.currentTarget.value)}
                />
              </Field>
              <Field
                label={t("settings:mail.clientSecret")}
                error={err("graph_client_secret")}
                tip={t("settings:mail.clientSecretHelp")}
              >
                <SecretInput
                  value={form.graph_client_secret}
                  saved={form.has_graph_client_secret}
                  onInput={(v) => setForm("graph_client_secret", v)}
                />
              </Field>
            </fieldset>
          </Show>

          <Show when={form.transport === "smtp"}>
            <fieldset class="st-group">
              <legend>{t("settings:mail.smtpServer")}</legend>
              <div class="st-row">
                <Field
                  label={t("settings:mail.server")}
                  error={err("smtp_host")}
                  tip={t("settings:mail.serverHelp")}
                >
                  <input
                    value={form.smtp_host}
                    placeholder="smtp.empresa.com"
                    onInput={(e) => setForm("smtp_host", e.currentTarget.value)}
                  />
                </Field>
                <Field
                  label={t("settings:mail.encryption")}
                  tip={t("settings:mail.encryptionHelp")}
                >
                  <select
                    value={form.smtp_security}
                    onChange={(e) => {
                      const security = e.currentTarget.value as SmtpSecurity;
                      // If the port was another encryption's, follow the new one.
                      if (Object.values(DEFAULT_PORT).includes(form.smtp_port)) {
                        setForm("smtp_port", DEFAULT_PORT[security]);
                      }
                      setForm("smtp_security", security);
                    }}
                  >
                    <option value="starttls">STARTTLS</option>
                    <option value="tls">{t("settings:mail.implicitTls")}</option>
                    <option value="none">{t("settings:mail.unencrypted")}</option>
                  </select>
                </Field>
                <Field
                  label={t("settings:mail.port")}
                  error={err("smtp_port")}
                  tip={t("settings:mail.portHelp")}
                >
                  <input
                    type="number"
                    min="1"
                    max="65535"
                    value={form.smtp_port}
                    onInput={(e) => setForm("smtp_port", Number(e.currentTarget.value) || 0)}
                  />
                </Field>
              </div>
              <div class="st-row">
                <Field
                  label={t("settings:mail.username")}
                  hint={t("settings:mail.usernamePlaceholder")}
                  tip={t("settings:mail.usernameHelp")}
                >
                  <input
                    value={form.smtp_username}
                    autocomplete="off"
                    onInput={(e) => setForm("smtp_username", e.currentTarget.value)}
                  />
                </Field>
                <Field
                  label={t("settings:mail.password")}
                  error={err("smtp_password")}
                  tip={t("settings:mail.passwordHelp")}
                >
                  <SecretInput
                    value={form.smtp_password}
                    saved={form.has_smtp_password}
                    onInput={(v) => setForm("smtp_password", v)}
                  />
                </Field>
              </div>
            </fieldset>
          </Show>

          <fieldset class="st-group">
            <legend>{t("settings:mail.alerts")}</legend>
            <Field
              label={t("settings:mail.alertLanguage")}
              error={err("notice_language")}
              tip={t("settings:mail.alertLanguageTip")}
            >
              <select
                value={form.notice_language || "es"}
                onChange={(e) => setForm("notice_language", e.currentTarget.value)}
              >
                <option value="es">Español</option>
                <option value="en">English</option>
              </select>
            </Field>
          </fieldset>

          <fieldset class="st-group">
            <legend>{t("settings:mail.passwordReset")}</legend>
            <Field
              label={t("settings:mail.resetLinkMinutes")}
              error={err("reset_link_minutes")}
              hint={t("settings:mail.resetLinkPlaceholder", {
                settings: settings()?.reset_link_default_minutes ?? 30,
              })}
              tip={t("settings:mail.resetLinkHelp")}
            >
              <input
                type="number"
                min="5"
                max="1440"
                value={form.reset_link_minutes || ""}
                placeholder={String(settings()?.reset_link_default_minutes ?? 30)}
                onInput={(e) => setForm("reset_link_minutes", Number(e.currentTarget.value) || 0)}
              />
            </Field>
          </fieldset>

          <NoticeBox notice={save.notice()} />
          <div class="st-actions">
            <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
              {save.busy() ? t("settings:mail.saving") : t("settings:mail.save")}
            </button>
          </div>
        </form>

        <div class="card st-form">
          <h2 class="st-subtitle">{t("settings:mail.sendTestEmail")}</h2>
          <p class="st-help">{t("settings:mail.usesForm")}</p>
          <div class="st-inline">
            <input
              type="email"
              value={testTo()}
              placeholder="tu@empresa.com"
              onInput={(e) => setTestTo(e.currentTarget.value)}
            />
            <button
              type="button"
              class="fk-btn"
              disabled={test.busy()}
              onClick={() =>
                test.run(async () => {
                  await settingsApi.testMail({ ...form, enabled: true }, testTo());
                  return t("settings:mail.testSent", { testTo: testTo() });
                })
              }
            >
              {test.busy() ? t("settings:mail.sending") : t("settings:mail.sendTest")}
            </button>
          </div>
          <Show when={test.fields().to}>
            <span class="st-field__error">{test.fields().to}</span>
          </Show>
          <NoticeBox notice={test.notice()} />
        </div>
      </Show>
    </section>
  );
};
