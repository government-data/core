import { A, useSearchParams } from "@solidjs/router";
import { createResource, For, Show, type Component, type JSX } from "solid-js";
import { formatDay, platformApi, type ContractStatus } from "./api";
import { tr } from "./i18n";
import { Icon, type IconName } from "./icons";
import "./platform.css";
import { toast } from "./toast";
import { updateNotice } from "./updateNotice";

/** Who I am and what I may do, shared by every view of the feature. */
const [me, { refetch: refetchMe }] = createResource(() => platformApi.me());
export { me, refetchMe };

export const ErrorBox: Component<{ error: unknown }> = (props) => (
  <Show when={props.error}>
    {(e) => (
      <div class="kn-error">{e() instanceof Error ? (e() as Error).message : String(e())}</div>
    )}
  </Show>
);

/**
 * How many days before the contract ends the UI warns. The cloud marks it as
 * «ending soon» earlier (60 days), for the warning emails to the
 * administrators (60, 30 and 7 days).
 */
export const CONTRACT_BANNER_DAYS = 3;

/** The contract in one sentence, with its tone (`null` if it is current). */
export function contractNotice(
  status: ContractStatus | null | undefined,
): { text: string; tone: "warn" | "bad" } | null {
  switch (status?.state) {
    case "ending_soon":
      if (status.days_left > CONTRACT_BANNER_DAYS) return null;
      return {
        text: tr("platform:common.contractExpiresOn", {
          endsAt: formatDay(status.ends_at),
          daysLeft: status.days_left,
        }),
        tone: "warn",
      };
    case "grace":
      return {
        text: tr("platform:common.contractExpired", {
          endedAt: formatDay(status.ended_at),
          readOnlyFrom: formatDay(status.read_only_from),
        }),
        tone: "warn",
      };
    case "read_only":
      return {
        text: tr("platform:common.readOnly", { reason: status.reason }),
        tone: "bad",
      };
    default:
      return null;
  }
}

/** The updates notice, above every view. */
const UpdateNoticeBanner: Component = () => (
  <Show when={updateNotice(me()?.update_notice)}>
    {(n) => (
      <div class={`kn-notice kn-notice--${n().tone}`} role="status">
        {n().text}{" "}
        <Show when={n().link}>
          <A href="/settings/updates">{tr("platform:common.viewUpdates")}</A>
        </Show>
      </div>
    )}
  </Show>
);

/** The contract notice, above every view (every user sees it). */
const LicenseNotice: Component = () => (
  <Show when={contractNotice(me()?.license_notice)}>
    {(n) => (
      <div class={`kn-notice kn-notice--${n().tone}`} role="status">
        {n().text}
      </div>
    )}
  </Show>
);

export const Page: Component<{
  title: string;
  subtitle?: string;
  /** Buttons to the right of the title, always in sight. */
  actions?: JSX.Element;
  /** Where to go back to: an arrow above the title, as in the lab. */
  back?: { href: string; label: string };
  children: JSX.Element;
}> = (props) => (
  <section class="feature">
    <Show when={props.back}>
      {(b) => (
        <A href={b().href} class="kn-back">
          <Icon name="back" size={16} stroke={1.5} />
          {b().label}
        </A>
      )}
    </Show>
    <header class="feature__header kn-page-head">
      <div class="kn-page-head__text">
        <h1 class="feature__title">{props.title}</h1>
        <Show when={props.subtitle}>
          <p class="feature__subtitle">{props.subtitle}</p>
        </Show>
      </div>
      <Show when={props.actions}>
        <div class="kn-page-head__actions">{props.actions}</div>
      </Show>
    </header>
    <LicenseNotice />
    <UpdateNoticeBanner />
    {props.children}
  </section>
);

/**
 * Tabs with an icon: shows only the chosen panel. The chosen one stays in the
 * URL (`?tab=`) so it can be linked and survives a reload.
 */
export function Tabs<K extends string>(props: {
  tabs: { key: K; label: string; icon: IconName; panel: () => JSX.Element }[];
}) {
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const current = () => props.tabs.find((t) => t.key === params.tab) ?? props.tabs[0];
  return (
    <>
      <div class="kn-tabs" role="tablist">
        <For each={props.tabs}>
          {(t) => (
            <button
              type="button"
              role="tab"
              class="kn-tabs__tab"
              aria-selected={current()?.key === t.key}
              onClick={() => setParams({ tab: t.key }, { replace: true })}
            >
              <Icon name={t.icon} />
              {t.label}
            </button>
          )}
        </For>
      </div>
      <div role="tabpanel">
        <Show when={current()} keyed>
          {(t) => t.panel()}
        </Show>
      </div>
    </>
  );
}

/**
 * Run an action: if it succeeds and there is a `success`, confirm it with a
 * toast; if it fails, show the error in a toast and leave it in `setError`
 * (for the forms, where the error must stay in sight next to the fields).
 */
export async function attempt(
  action: () => Promise<unknown>,
  setError: (e: unknown) => void,
  success?: string,
): Promise<boolean> {
  setError(undefined);
  try {
    await action();
    if (success) toast("success", success);
    return true;
  } catch (e) {
    setError(e);
    toast(
      "error",
      tr("platform:common.couldNotComplete"),
      e instanceof Error ? e.message : String(e),
    );
    return false;
  }
}
