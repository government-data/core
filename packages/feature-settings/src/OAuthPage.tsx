/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { settingsApi, type OAuthProvider, type Source } from "./api";
import {
  CopyButton,
  Field,
  NoticeBox,
  SecretInput,
  SectionHeader,
  SourceNote,
  Toggle,
  useAction,
} from "./common";
import { t, translated } from "./i18n";

interface Preset {
  label: string;
  id: string;
  issuer: (tenant: string) => string;
  needsTenant?: boolean;
  scopes: string[];
}

const PRESETS: Preset[] = translated(() => [
  {
    label: t("settings:oauth.microsoftEntraId"),
    id: "microsoft",
    issuer: (tenant) => `https://login.microsoftonline.com/${tenant}/v2.0`,
    needsTenant: true,
    scopes: ["openid", "email", "profile"],
  },
  {
    label: t("settings:oauth.google"),
    id: "google",
    issuer: () => "https://accounts.google.com",
    scopes: ["openid", "email", "profile"],
  },
  {
    label: t("settings:oauth.otherOidc"),
    id: "",
    issuer: () => "",
    scopes: ["openid", "email", "profile"],
  },
]);

/** The Entra ID tenants that accept accounts of any organization (or
 * personal ones): with them the email proves nothing. */
const SHARED_TENANTS = ["common", "organizations", "consumers"];

const blank = (id: string, issuer: string, scopes: string[]): OAuthProvider => ({
  id,
  enabled: true,
  client_id: "",
  client_secret: "",
  issuer,
  auth_endpoint: "",
  token_endpoint: "",
  userinfo_endpoint: "",
  scopes,
  trust_email: false,
});

export const OAuthPage: Component = () => {
  const [providers, setProviders] = createStore<OAuthProvider[]>([]);
  const [source, setSource] = createSignal<Source>();
  const [template, setTemplate] = createSignal("");
  // The common account the instance signs in with (`null` without it; not
  // read yet, `undefined`): with it gestión handles the access.
  const [account, setAccount] = createSignal<string | null>();
  const [tenant, setTenant] = createSignal("");
  const [discovering, setDiscovering] = createSignal<number>();
  // Which provider the last discovery notice belongs to (none: the notice is
  // the add form's).
  const [noticeFor, setNoticeFor] = createSignal<number>();
  const save = useAction();
  const discover = useAction();

  const load = (list: OAuthProvider[], src: Source) => {
    setProviders(reconcile(list.map((p) => ({ ...p }))));
    setSource(src);
  };

  const [settings] = createResource(async () => {
    const all = await settingsApi.all();
    load(all.oauth.value.providers, all.oauth.source);
    setTemplate(all.oauth_redirect_template);
    setAccount(all.platform_account ?? null);
    return all;
  });

  const redirectFor = (p: OAuthProvider) =>
    p.redirect_uri && p.redirect_uri.endsWith(`/${p.id}/callback`)
      ? p.redirect_uri
      : template().replace("{id}", p.id || "{id}");

  const err = (i: number, f: string) => save.fields()[`providers.${i}.${f}`];

  const add = (preset: Preset) => {
    if (preset.needsTenant && !tenant().trim()) {
      setNoticeFor(undefined);
      void discover.run(async () => {
        throw new Error(t("settings:oauth.tenantIdRequired"));
      });
      return;
    }
    const taken = new Set(providers.map((p) => p.id));
    let id = preset.id;
    for (let n = 2; id && taken.has(id); n++) id = `${preset.id}-${n}`;
    const own = tenant().trim();
    setProviders(providers.length, {
      ...blank(id, preset.issuer(own), preset.scopes),
      // Entra ID does not send `email_verified`; with the company's own tenant
      // it only hands out its own emails, so it is trusted (it can be removed).
      trust_email: preset.needsTenant === true && !SHARED_TENANTS.includes(own.toLowerCase()),
    });
    const index = providers.length - 1;
    if (providers[index].issuer) void runDiscovery(index);
  };

  const runDiscovery = (i: number) => {
    setDiscovering(i);
    setNoticeFor(i);
    return discover
      .run(async () => {
        const d = await settingsApi.discover(providers[i].issuer);
        setProviders(
          i,
          produce((p) => {
            p.issuer = d.issuer;
            p.auth_endpoint = d.auth_endpoint;
            p.token_endpoint = d.token_endpoint;
            p.userinfo_endpoint = d.userinfo_endpoint;
          }),
        );
        return t("settings:oauth.endpointsDiscovered");
      })
      .finally(() => setDiscovering(undefined));
  };

  return (
    <section class="feature">
      <SectionHeader title={t("settings:oauth.title")} subtitle={t("settings:oauth.intro")} />
      <Show when={settings.error}>
        <p class="st-notice st-notice--error">{t("settings:oauth.readError")}</p>
      </Show>
      <Show when={account()}>
        <p class="st-notice">{t("settings:oauth.managedByAccount")}</p>
      </Show>
      <Show when={account() === null}>
        <Show when={source()}>
          {(src) => (
            <SourceNote
              source={src()}
              onReset={() =>
                save.run(async () => {
                  const all = await settingsApi.reset("oauth");
                  load(all.oauth.value.providers, all.oauth.source);
                  return t("settings:oauth.switchedToEnv");
                })
              }
            />
          )}
        </Show>

        <div class="card st-form">
          <h2 class="st-subtitle">{t("settings:oauth.addProvider")}</h2>
          <div class="st-inline">
            <input
              value={tenant()}
              placeholder={t("settings:oauth.tenantId")}
              onInput={(e) => setTenant(e.currentTarget.value)}
            />
          </div>
          <div class="st-actions st-actions--start">
            <For each={PRESETS}>
              {(preset) => (
                <button type="button" class="fk-btn" onClick={() => add(preset)}>
                  + {preset.label}
                </button>
              )}
            </For>
          </div>
          <NoticeBox notice={noticeFor() === undefined ? discover.notice() : undefined} />
        </div>

        <form
          class="st-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void save.run(async () => {
              const saved = await settingsApi.saveOAuth({ providers: [...providers] });
              load(saved.value.providers, saved.source);
              const on = saved.value.providers.filter((p) => p.enabled).length;
              return t("settings:oauth.savedProviders", { count: on });
            });
          }}
        >
          <Show
            when={providers.length > 0}
            fallback={<p class="card st-empty">{t("settings:oauth.noProviders")}</p>}
          >
            <For each={providers}>
              {(p, i) => (
                <fieldset class="card st-form st-provider">
                  <div class="st-provider__head">
                    <Toggle checked={p.enabled} onChange={(v) => setProviders(i(), "enabled", v)}>
                      <strong>{p.id || t("settings:oauth.newProvider")}</strong>
                    </Toggle>
                    <button
                      type="button"
                      class="fk-btn fk-btn--sm fk-btn--danger"
                      onClick={() => {
                        setNoticeFor(undefined);
                        setProviders((list) => list.filter((_, j) => j !== i()));
                      }}
                    >
                      {t("settings:oauth.remove")}
                    </button>
                  </div>
                  <div class="st-row">
                    <Field
                      label={t("settings:oauth.id")}
                      error={err(i(), "id")}
                      tip={t("settings:oauth.nameHelp")}
                      hint={t("settings:oauth.slugHelp")}
                    >
                      <input
                        value={p.id}
                        onInput={(e) =>
                          setProviders(i(), "id", e.currentTarget.value.toLowerCase())
                        }
                      />
                    </Field>
                    <Field
                      label={t("settings:oauth.redirectUrl")}
                      tip={t("settings:oauth.redirectUrlHelp")}
                    >
                      <div class="st-inline">
                        <input readOnly value={redirectFor(p)} />
                        <CopyButton text={redirectFor(p)} />
                      </div>
                    </Field>
                  </div>
                  <div class="st-row">
                    <Field
                      label={t("settings:oauth.clientId")}
                      error={err(i(), "client_id")}
                      tip={t("settings:oauth.clientIdHelp")}
                    >
                      <input
                        value={p.client_id}
                        onInput={(e) => setProviders(i(), "client_id", e.currentTarget.value)}
                      />
                    </Field>
                    <Field
                      label={t("settings:oauth.clientSecret")}
                      error={err(i(), "client_secret")}
                      tip={t("settings:oauth.clientSecretHelp")}
                    >
                      <SecretInput
                        value={p.client_secret}
                        saved={p.has_client_secret}
                        onInput={(v) => setProviders(i(), "client_secret", v)}
                      />
                    </Field>
                  </div>
                  <Field
                    label={t("settings:oauth.issuer")}
                    error={err(i(), "issuer")}
                    tip={t("settings:oauth.microsoftHttpsLoginMicrosoftonline")}
                  >
                    <div class="st-inline">
                      <input
                        value={p.issuer}
                        placeholder="https://login.microsoftonline.com/<tenant>/v2.0"
                        onInput={(e) => setProviders(i(), "issuer", e.currentTarget.value)}
                      />
                      <button
                        type="button"
                        class="fk-btn"
                        disabled={discovering() !== undefined || !p.issuer}
                        onClick={() => runDiscovery(i())}
                      >
                        {discovering() === i()
                          ? t("settings:oauth.reading")
                          : t("settings:oauth.discoverEndpoints")}
                      </button>
                    </div>
                  </Field>
                  <NoticeBox notice={noticeFor() === i() ? discover.notice() : undefined} />
                  <Toggle
                    checked={p.trust_email}
                    onChange={(v) => setProviders(i(), "trust_email", v)}
                    tip={t("settings:oauth.trustEmailTip")}
                  >
                    {t("settings:oauth.trustEmail")}
                  </Toggle>
                  <details class="st-howto" open={!p.auth_endpoint}>
                    <summary>{t("settings:oauth.endpointsScopes")}</summary>
                    <Field
                      label={t("settings:oauth.authorization")}
                      error={err(i(), "auth_endpoint")}
                    >
                      <input
                        value={p.auth_endpoint}
                        onInput={(e) => setProviders(i(), "auth_endpoint", e.currentTarget.value)}
                      />
                    </Field>
                    <Field label={t("settings:oauth.token")} error={err(i(), "token_endpoint")}>
                      <input
                        value={p.token_endpoint}
                        onInput={(e) => setProviders(i(), "token_endpoint", e.currentTarget.value)}
                      />
                    </Field>
                    <Field
                      label={t("settings:oauth.userinfo")}
                      error={err(i(), "userinfo_endpoint")}
                    >
                      <input
                        value={p.userinfo_endpoint}
                        onInput={(e) =>
                          setProviders(i(), "userinfo_endpoint", e.currentTarget.value)
                        }
                      />
                    </Field>
                    <Field
                      label={t("settings:oauth.scopes")}
                      hint={t("settings:oauth.separatedSpaces")}
                      tip={t("settings:oauth.scopesHelp")}
                    >
                      <input
                        value={p.scopes.join(" ")}
                        onInput={(e) =>
                          setProviders(
                            i(),
                            "scopes",
                            e.currentTarget.value.split(/[\s,]+/).filter(Boolean),
                          )
                        }
                      />
                    </Field>
                  </details>
                </fieldset>
              )}
            </For>
          </Show>

          <div class="card st-form">
            <NoticeBox notice={save.notice()} />
            <div class="st-actions">
              <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
                {save.busy() ? t("settings:oauth.saving") : t("settings:oauth.saveProviders")}
              </button>
            </div>
          </div>
        </form>
      </Show>
    </section>
  );
};
