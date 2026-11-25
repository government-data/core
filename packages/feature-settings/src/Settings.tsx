/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { createResource, For, Show, type Component } from "solid-js";
import { Dynamic } from "solid-js/web";
import { A } from "@solidjs/router";
import { useSettingsCards } from "@knime-orchestador/feature-kit";
import { accountApi, may, meApi, secretsApi, settingsApi, usersApi } from "./api";
import "./settings.css";
import { t } from "./i18n";
import { CardIcon } from "./CardIcon";
import { SECTIONS } from "./homeSections";

/** The updates in one line (`null` if it is not updated from here). */
interface UpdatesSummary {
  stale: boolean;
  daemon: boolean;
  report: { phase: string; installed: string; available: { version: string } | null };
}

async function updatesSummary(): Promise<{ on: boolean; text: string } | null> {
  const res = await fetch("/api/updates", { credentials: "same-origin" });
  if (!res.ok) return null;
  const info = (await res.json()) as UpdatesSummary | null;
  if (!info) return null;
  if (!info.daemon) return { on: false, text: t("settings:home.updates.noDaemon") };
  if (info.stale) return { on: false, text: t("settings:home.updates.stale") };
  const { installed, available, phase } = info.report;
  if (phase && phase !== "idle" && phase !== "prepared")
    return { on: true, text: t("settings:home.updates.updating") };
  return available
    ? {
        on: true,
        text: t("settings:home.updates.available", { installed, version: available.version }),
      }
    : { on: true, text: t("settings:home.updates.current", { installed }) };
}

/** «Mi cuenta»: each person's profile and, if there are OAuth providers, their
 * linked accounts. */
async function accountSummary(): Promise<{ enabled: boolean; on: boolean; text: string } | null> {
  const providers = await accountApi.providers();
  if (providers.length === 0) {
    return { enabled: true, on: true, text: t("settings:home.account.profile") };
  }
  const linked = await accountApi.identities().catch(() => []);
  return {
    enabled: true,
    on: linked.length > 0,
    text:
      linked.length > 0
        ? t("settings:home.account.linked", { count: linked.length })
        : t("settings:home.account.none"),
  };
}

/** The users in one line: how many and how many still have to activate. */
async function usersSummary(): Promise<{ on: boolean; text: string }> {
  const { users } = await usersApi.list();
  const pending = users.filter((u) => u.status === "pending").length;
  return {
    on: true,
    text:
      t("settings:home.users.count", { count: users.length }) +
      (pending > 0 ? t("settings:home.users.pending", { count: pending }) : ""),
  };
}

/** The global secrets in one line: how many there are. */
async function secretsSummary(): Promise<{ on: boolean; text: string }> {
  const secrets = await secretsApi.list();
  return {
    on: secrets.length > 0,
    text:
      secrets.length > 0
        ? t("settings:home.secrets.count", { count: secrets.length })
        : t("settings:home.secrets.none"),
  };
}

export const Settings: Component = () => {
  // Each card for whoever may do what it leads to (ADR 0016); everyone sees
  // their account and their service accounts.
  const [me] = createResource(meApi);
  const can = (action: string) => may(me(), action);
  const [settings] = createResource(
    () => can("admin:Configure"),
    () => settingsApi.all(),
  );
  const [updates] = createResource(() => can("admin:ManageUpdates"), updatesSummary);
  const [account] = createResource(accountSummary);
  const [users] = createResource(() => can("admin:ManageUsers"), usersSummary);
  const [secrets] = createResource(() => can("admin:ManageSecrets"), secretsSummary);
  const manages = () => (me()?.actions.length ?? 0) > 0;

  return (
    <section class="feature">
      <header class="feature__header">
        <h1 class="feature__title">{t("settings:home.title")}</h1>
        <p class="feature__subtitle">
          {manages() ? t("settings:home.subtitle") : t("settings:home.subtitleUser")}
        </p>
      </header>
      <Show when={settings.error}>
        <p class="st-notice st-notice--error">{t("settings:home.readError")}</p>
      </Show>
      <Show when={settings()?.encryption_key === "generated"}>
        <p class="st-notice st-notice--warn">{t("settings:home.keyGenerated")}</p>
      </Show>
      <div class="settings-grid">
        <Show when={account()}>
          {(a) => (
            <Dynamic
              component={a().enabled ? A : "div"}
              href="/settings/account"
              class="settings-card"
              classList={{ "settings-card--disabled": !a().enabled }}
              aria-disabled={!a().enabled}
            >
              <span class="settings-card__icon">
                <CardIcon name="link" />
              </span>
              <div class="settings-card__body">
                <span class="settings-card__label">{t("settings:home.account.label")}</span>
                <span class="settings-card__desc">{t("settings:home.account.desc")}</span>
                <span class="badge" classList={{ "badge--muted": !a().on }}>
                  {a().text}
                </span>
              </div>
            </Dynamic>
          )}
        </Show>
        <A href="/settings/access" class="settings-card">
          <span class="settings-card__icon">
            <CardIcon name="access" />
          </span>
          <div class="settings-card__body">
            <span class="settings-card__label">{t("settings:home.access.label")}</span>
            <span class="settings-card__desc">
              {can("admin:ManageUsers") || can("admin:ManagePolicies")
                ? t("settings:home.access.desc")
                : t("settings:home.access.descOwn")}
            </span>
            <Show when={users()}>{(u) => <span class="badge">{u().text}</span>}</Show>
          </div>
        </A>
        <Show when={can("admin:ManageSecrets")}>
          <A href="/settings/secrets" class="settings-card">
            <span class="settings-card__icon">
              <CardIcon name="variables" />
            </span>
            <div class="settings-card__body">
              <span class="settings-card__label">{t("settings:home.secrets.label")}</span>
              <span class="settings-card__desc">{t("settings:home.secrets.desc")}</span>
              <Show when={secrets()}>
                {(s) => (
                  <span class="badge" classList={{ "badge--muted": !s().on }}>
                    {s().text}
                  </span>
                )}
              </Show>
            </div>
          </A>
        </Show>
        <Show when={updates()}>
          {(u) => (
            <A href="/settings/updates" class="settings-card">
              <span class="settings-card__icon">
                <CardIcon name="updates" />
              </span>
              <div class="settings-card__body">
                <span class="settings-card__label">{t("settings:home.updates.label")}</span>
                <span class="settings-card__desc">{t("settings:home.updates.desc")}</span>
                <span class="badge" classList={{ "badge--muted": !u().on }}>
                  {u().text}
                </span>
              </div>
            </A>
          )}
        </Show>
        {/* What the other features add (e.g. Airflow, from the Python engine). */}
        <For each={useSettingsCards()}>
          {(card) => (
            <Show when={card.visible?.() ?? true}>
              <A href={card.path} class="settings-card">
                <Show when={card.icon}>
                  {(icon) => (
                    <span class="settings-card__icon">
                      <Dynamic component={icon()} size={22} />
                    </span>
                  )}
                </Show>
                <div class="settings-card__body">
                  <span class="settings-card__label">{card.label}</span>
                  <span class="settings-card__desc">{card.description}</span>
                </div>
              </A>
            </Show>
          )}
        </For>
        <For each={SECTIONS}>
          {(section) => (
            <Show
              when={
                can("admin:Configure") &&
                (settings.state === "errored" || settings.state === "ready")
              }
            >
              <A href={section.path} class="settings-card">
                <span class="settings-card__icon">
                  <CardIcon name={section.icon} />
                </span>
                <div class="settings-card__body">
                  <span class="settings-card__label">{section.label}</span>
                  <span class="settings-card__desc">{section.description}</span>
                  <Show when={settings()}>
                    {(s) => {
                      const status = () => section.status(s());
                      return (
                        <span class="badge" classList={{ "badge--muted": !status().on }}>
                          {status().text}
                        </span>
                      );
                    }}
                  </Show>
                </div>
              </A>
            </Show>
          )}
        </For>
      </div>
    </section>
  );
};
