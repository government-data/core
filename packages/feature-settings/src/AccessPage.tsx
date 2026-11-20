import "./settings.css";
import { createResource, For, Show, type Component } from "solid-js";
import { Dynamic } from "solid-js/web";
import { useSearchParams } from "@solidjs/router";
import { may, meApi, type Me } from "./api";
import { SectionHeader } from "./common";
import { t } from "./i18n";
import { CardIcon, type CardIconName } from "./CardIcon";
import { GroupsPage } from "./GroupsPage";
import { PoliciesPage } from "./PoliciesPage";
import { ServiceAccountsPage } from "./ServiceAccountsPage";
import { UsersPage } from "./UsersPage";

interface Tab {
  key: string;
  icon: CardIconName;
  panel: Component<{ embedded?: boolean }>;
  /** Whether `me` sees it (the API checks the same). */
  shown: (me: Me) => boolean;
}

const TABS: Tab[] = [
  { key: "users", icon: "users", panel: UsersPage, shown: (me) => may(me, "admin:ManageUsers") },
  {
    key: "groups",
    icon: "group",
    panel: GroupsPage,
    shown: (me) => may(me, "admin:ManagePolicies"),
  },
  {
    key: "policies",
    icon: "policy",
    panel: PoliciesPage,
    shown: (me) => may(me, "admin:ManagePolicies"),
  },
  // Everyone has their own; a service account does not make others.
  {
    key: "service-accounts",
    icon: "key",
    panel: ServiceAccountsPage,
    shown: (me) => !me.service_account,
  },
];

/**
 * Settings → Users and access (ADR 0016): the accounts, the groups, the
 * policies and the service accounts in one place, each in its tab. The
 * chosen one stays in the URL (`?tab=`), so it can be linked.
 */
export const AccessPage: Component = () => {
  const [me] = createResource(meApi);
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const tabs = () => {
    const m = me();
    return m ? TABS.filter((tab) => tab.shown(m)) : [];
  };
  const current = () => tabs().find((tab) => tab.key === params.tab) ?? tabs()[0];

  return (
    <section class="feature">
      <SectionHeader title={t("settings:access.title")} subtitle={t("settings:access.subtitle")} />
      <Show when={tabs().length > 0}>
        <div class="st-tabs" role="tablist">
          <For each={tabs()}>
            {(tab) => (
              <button
                type="button"
                role="tab"
                class="st-tabs__tab"
                aria-selected={current()?.key === tab.key}
                onClick={() => setParams({ tab: tab.key }, { replace: true })}
              >
                <CardIcon name={tab.icon} size={18} />
                {t(`settings:access.tab.${tab.key}`)}
              </button>
            )}
          </For>
        </div>
        <div role="tabpanel">
          <Show when={current()} keyed>
            {(tab) => <Dynamic component={tab.panel} embedded />}
          </Show>
        </div>
      </Show>
    </section>
  );
};
