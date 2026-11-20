/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { A } from "@solidjs/router";
import { createEffect, createResource, createSignal, For, Show, type Component } from "solid-js";
import { formatNumber, locale } from "@knime-orchestador/i18n";
import {
  formatDate,
  formatDay,
  platformApi,
  type LicenseBilling,
  type LicenseInfo,
  type LicenseLimits,
  type LicenseUsage,
} from "./api";
import { attempt, contractNotice, ErrorBox, Page } from "./common";
import { tr, translated } from "./i18n";
import { Icon } from "./icons";

/** What is shown of each limit of the plan. */
const ROWS: { key: keyof LicenseLimits & keyof LicenseUsage; label: string; note?: string }[] =
  translated(() => [
    { key: "servers", label: tr("platform:license.servers") },
    { key: "workflows", label: tr("platform:license.workflows") },
    {
      key: "users",
      label: tr("platform:license.users"),
      note: tr("platform:license.notCountingAdmins"),
    },
    {
      key: "executions_month",
      label: tr("platform:license.runsThisMonth"),
      note: tr("platform:license.neverStopped"),
    },
    { key: "deployments_month", label: tr("platform:license.deploymentsThisMonth") },
  ]);

const FEATURES: Record<string, string> = translated(() => ({
  ssh: tr("platform:license.sshMachineAdmin"),
  agent_link: tr("platform:license.agentInstallerByLink"),
  ldap: tr("platform:license.ldapAreasByGroup"),
  sso: tr("platform:license.singleSignOn"),
  mail: tr("platform:license.notificationEmail"),
  ai: tr("platform:license.aiDiagnostics"),
}));

/** The license's state, in one line. */
function statusLine(info: LicenseInfo): { text: string; tone: "ok" | "warn" | "bad" } {
  const s = info.license.status;
  // With a valid entitlement, what counts is the contract.
  const contract = contractNotice(info.license.contract);
  if (s.state !== "basic" && contract) return contract;
  if (s.state === "active") {
    const ends = info.license.entitlement?.contract_ends_at;
    return {
      text: ends
        ? tr("platform:license.activeUntil", { formatDay: formatDay(ends) })
        : tr("platform:license.activeNoExpiration"),
      tone: "ok",
    };
  }
  if (s.state === "grace")
    return {
      text: tr("platform:license.notRenewed", { until: formatDate(s.until) }),
      tone: "warn",
    };
  return { text: tr("platform:license.basicPlan", { reason: s.reason }), tone: "bad" };
}

const cop = (value: number) =>
  formatNumber(value, { style: "currency", currency: "COP", maximumFractionDigits: 0 });

/** A day without a time (`2026-10-15`): at noon, so no time zone shifts it. */
const paidOn = (day: string) =>
  new Intl.DateTimeFormat(locale(), { dateStyle: "medium" }).format(new Date(`${day}T12:00:00`));

const PERIODS: Record<string, string> = translated(() => ({
  mensual: tr("platform:license.monthly"),
  semestral: tr("platform:license.semiannual"),
  anual: tr("platform:license.annual"),
}));

/** The payments and the renewal, asked of the license cloud. */
const Billing: Component = () => {
  const [data] = createResource<LicenseBilling>(() => platformApi.licenseBilling());
  return (
    <>
      <h2 class="kn-section">{tr("platform:license.billing")}</h2>
      <ErrorBox error={data.error} />
      <Show when={data()}>
        {(d) => (
          <div class="card kn-form">
            <Show when={d().error}>
              <div class="kn-license__error">
                {d().billing
                  ? tr("platform:license.billingCloudDown", {
                      fetched: formatDate(d().fetched_at),
                      error: d().error,
                    })
                  : d().error}
              </div>
            </Show>
            <Show when={d().billing}>
              {(b) => (
                <>
                  <dl class="kn-dl">
                    <dt>{tr("platform:license.period")}</dt>
                    <dd>{b().period ? (PERIODS[b().period!] ?? b().period) : "—"}</dd>
                    <Show when={b().renewal_cop}>
                      <dt>{tr("platform:license.renewalAmount")}</dt>
                      <dd>{cop(b().renewal_cop!)}</dd>
                    </Show>
                  </dl>
                  <p class="table__muted">{tr("platform:license.howToRenew")}</p>
                  <Show
                    when={b().payments.length}
                    fallback={
                      <div class="kn-empty">{tr("platform:license.noPaymentsRecordedYet")}</div>
                    }
                  >
                    <div class="kn-table-wrap">
                      <table class="table">
                        <thead>
                          <tr>
                            <th>{tr("platform:license.invoice")}</th>
                            <th>{tr("platform:license.paidOn")}</th>
                            <th>{tr("platform:license.covers")}</th>
                            <th>{tr("platform:license.amount")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          <For each={b().payments}>
                            {(p) => (
                              <tr>
                                <td class="kn-mono">{p.invoice}</td>
                                <td>{paidOn(p.paid_on)}</td>
                                <td>
                                  {formatDay(p.from)} – {formatDay(p.to)}
                                  <div class="table__muted">{PERIODS[p.period] ?? p.period}</div>
                                </td>
                                <td>{cop(p.amount_cop)}</td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </div>
                  </Show>
                </>
              )}
            </Show>
          </div>
        )}
      </Show>
    </>
  );
};

/** The common account this installation signs in with (`https://cuenta.…`), or `null`. */
async function accountUrl(): Promise<string | null> {
  const r = await fetch("/api/settings/account", { credentials: "same-origin" }).catch(() => null);
  if (!r?.ok) return null;
  const { url } = (await r.json()) as { url: string | null };
  return url && url.startsWith("https://") ? url : null;
}

/** The installation's license: plan, usage of its limits and the key (admin only). */
export const License: Component = () => {
  const [info, { refetch }] = createResource(() => platformApi.license());
  const [error, setError] = createSignal<unknown>();
  const [changing, setChanging] = createSignal(false);
  const [key, setKey] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  // With the common account, the license lives in its Billing (plan, usage,
  // features, payments and key): it goes there. With ?here (the link to
  // change the key from the Billing), it stays.
  const here = new URLSearchParams(window.location.search).has("here");
  const [account] = createResource(async () => (here ? null : await accountUrl()));
  createEffect(() => {
    const base = account();
    if (base) window.location.replace(`${base}/billing`);
  });
  const stay = () => here || (account.state === "ready" && !account());

  const run = (action: () => Promise<unknown>, success: string) =>
    attempt(
      async () => {
        setBusy(true);
        try {
          await action();
          setChanging(false);
          setKey("");
        } finally {
          setBusy(false);
          await refetch();
        }
      },
      setError,
      success,
    );

  return (
    <Show when={stay()}>
      <Page title={tr("platform:license.license")} subtitle={tr("platform:license.intro")}>
        <A href="/settings" class="kn-back">
          <Icon name="back" size={16} stroke={1.5} />
          {tr("platform:license.settings")}
        </A>
        <ErrorBox error={error() ?? info.error} />
        <Show
          when={info()}
          fallback={
            <Show when={!info.loading}>
              <div class="card kn-empty">{tr("platform:license.noLicenseUsed")}</div>
            </Show>
          }
        >
          {(i) => {
            const line = () => statusLine(i());
            const e = () => i().license.entitlement;
            return (
              <>
                <Show when={i().license.dev_key}>
                  <div class="kn-error">{tr("platform:license.devKeyBinary")}</div>
                </Show>
                <div class="card kn-form">
                  <div class={`kn-license__status kn-license__status--${line().tone}`}>
                    {line().text}
                  </div>
                  <dl class="kn-dl">
                    <dt>{tr("platform:license.customer")}</dt>
                    <dd>{e()?.customer ?? "—"}</dd>
                    <dt>{tr("platform:license.plan")}</dt>
                    <dd>{e()?.plan ?? tr("platform:license.basic")}</dd>
                    <dt>{tr("platform:license.key")}</dt>
                    <dd class="kn-mono">{i().license.key_hint ?? "sin clave"}</dd>
                    <dt>{tr("platform:license.installation")}</dt>
                    <dd class="kn-mono">{i().license.installation}</dd>
                    <dt>{tr("platform:license.lastRenewal")}</dt>
                    <dd>{formatDate(i().license.fetched_at)}</dd>
                    <Show when={i().license.last_error}>
                      <dt>{tr("platform:license.lastAttempt")}</dt>
                      <dd class="kn-license__error">
                        {formatDate(i().license.last_attempt_at)} · {i().license.last_error}
                      </dd>
                    </Show>
                  </dl>
                  <div class="kn-form__actions">
                    <button
                      class="fk-btn"
                      disabled={busy()}
                      onClick={() =>
                        void run(
                          () => platformApi.refreshLicense(),
                          tr("platform:license.licenseRenewed"),
                        )
                      }
                    >
                      {tr("platform:license.renewNow")}
                    </button>
                    <button
                      class="fk-btn"
                      disabled={busy()}
                      onClick={() => setChanging(!changing())}
                    >
                      {tr("platform:license.changeKey")}
                    </button>
                  </div>
                  <Show when={changing()}>
                    <form
                      class="kn-form__row"
                      onSubmit={(ev) => {
                        ev.preventDefault();
                        void run(
                          () => platformApi.setLicenseKey(key()),
                          tr("platform:license.keyActivated"),
                        );
                      }}
                    >
                      <label>
                        {tr("platform:license.newActivationKey")}
                        <input
                          value={key()}
                          onInput={(ev) => setKey(ev.currentTarget.value)}
                          placeholder="ORQ-XXXX-XXXX"
                          required
                        />
                      </label>
                      <button
                        type="submit"
                        class="fk-btn kn-primary"
                        disabled={busy() || !key().trim()}
                      >
                        {tr("platform:license.activate")}
                      </button>
                    </form>
                  </Show>
                </div>

                <h2 class="kn-section">{tr("platform:license.planUsage")}</h2>
                <div class="card kn-table-wrap">
                  <table class="table">
                    <thead>
                      <tr>
                        <th>{tr("platform:license.limit")}</th>
                        <th>{tr("platform:license.usageColumn")}</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      <For each={ROWS}>
                        {(row) => {
                          const used = () => i().usage[row.key];
                          const limit = () => i().license.limits[row.key];
                          const pct = () =>
                            limit() ? Math.min(100, (used() / limit()!) * 100) : 0;
                          return (
                            <tr>
                              <td>
                                {row.label}
                                <Show when={row.note}>
                                  <div class="table__muted">{row.note}</div>
                                </Show>
                              </td>
                              <td>
                                {tr("platform:license.usage", {
                                  used: used(),
                                  limit: limit() ?? tr("platform:license.unlimited"),
                                })}
                              </td>
                              <td class="kn-license__bar">
                                <Show when={limit()}>
                                  <progress
                                    max={100}
                                    value={pct()}
                                    classList={{
                                      "kn-license--warn": pct() >= 80 && pct() < 100,
                                      "kn-license--full": pct() >= 100,
                                    }}
                                  />
                                </Show>
                              </td>
                            </tr>
                          );
                        }}
                      </For>
                    </tbody>
                  </table>
                </div>

                <Show when={i().license.key_hint}>
                  <Billing />
                </Show>

                <h2 class="kn-section">{tr("platform:license.features")}</h2>
                <div class="card">
                  <ul class="kn-license__features">
                    <For each={Object.entries(FEATURES)}>
                      {([id, label]) => {
                        const on = () => !!e()?.features.includes(id) && line().tone !== "bad";
                        return (
                          <li classList={{ "kn-license__off": !on() }}>
                            {on() ? "✓" : "—"} {label}
                          </li>
                        );
                      }}
                    </For>
                  </ul>
                </div>
              </>
            );
          }}
        </Show>
      </Page>
    </Show>
  );
};
