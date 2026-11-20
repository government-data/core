import { Navigate } from "@solidjs/router";
import { createResource, createRoot, lazy, type Component, type Resource } from "solid-js";
import { defineFeature } from "@knime-orchestador/feature-kit";
import { may, meApi, type Me } from "./api";
import { CardIcon } from "./CardIcon";
import { t } from "./i18n";
import { SettingsNavIcon } from "./NavIcon";

export { SettingsNavIcon };

const view = <M,>(load: () => Promise<M>, pick: (m: M) => Component) =>
  lazy(() => load().then((m) => ({ default: pick(m) })));

// What the person may do, read the first time Settings' home asks for the
// cards and owned by a root of its own; the API checks again.
let actions: Resource<Me> | undefined;
const who = () => (actions ??= createRoot(() => createResource(meApi)[0]))();

/**
 * Settings, reached from the account's menu at the top right, not the
 * sidebar (decision 0043): the home page, «Mi cuenta» and «Usuarios y acceso» (whose tabs
 * show by action; the service accounts, for anyone), and the system part ({@link settingsAdminFeature}): each page
 * for whoever may do its action (ADR 0016), which the API checks.
 */
export const settingsFeature = defineFeature({
  id: "settings",
  get title() {
    return t("settings:nav");
  },
  routes: [
    {
      path: "/settings",
      component: view(
        () => import("./Settings"),
        (m) => m.Settings,
      ),
    },
    {
      path: "/settings/access",
      component: view(
        () => import("./AccessPage"),
        (m) => m.AccessPage,
      ),
    },
    {
      path: "/settings/account",
      component: view(
        () => import("./AccountPage"),
        (m) => m.AccountPage,
      ),
    },
    {
      path: "/settings/service-accounts",
      component: () => <Navigate href="/settings/access?tab=service-accounts" />,
    },
  ],
});

/** The system part of Settings: its cards show by action, and the API
 * refuses whoever may not. */
export const settingsAdminFeature = defineFeature({
  id: "settings-admin",
  get title() {
    return t("settings:nav");
  },
  routes: [
    { path: "/settings/groups", component: () => <Navigate href="/settings/access?tab=groups" /> },
    {
      path: "/settings/policies",
      component: () => <Navigate href="/settings/access?tab=policies" />,
    },
    { path: "/settings/users", component: () => <Navigate href="/settings/access?tab=users" /> },
    {
      path: "/settings/secrets",
      component: view(
        () => import("./SecretsPage"),
        (m) => m.SecretsPage,
      ),
    },
    {
      path: "/settings/ldap",
      component: view(
        () => import("./LdapPage"),
        (m) => m.LdapPage,
      ),
    },
    {
      path: "/settings/mail",
      component: view(
        () => import("./MailPage"),
        (m) => m.MailPage,
      ),
    },
    {
      path: "/settings/oauth",
      component: view(
        () => import("./OAuthPage"),
        (m) => m.OAuthPage,
      ),
    },
    {
      path: "/settings/execution",
      component: view(
        () => import("./ExecutionPage"),
        (m) => m.ExecutionPage,
      ),
    },
    {
      path: "/settings/lake",
      component: view(
        () => import("./LakePage"),
        (m) => m.LakePage,
      ),
    },
  ],
  settingsCards: [
    {
      path: "/settings/execution",
      get label() {
        return t("settings:machines.card");
      },
      get description() {
        return t("settings:machines.cardDesc");
      },
      icon: (p) => <CardIcon name="server" size={p.size} />,
      visible: () => may(who(), "admin:ManageMachines"),
    },
    {
      path: "/settings/lake",
      get label() {
        return t("settings:lake.card");
      },
      get description() {
        return t("settings:lake.cardDesc");
      },
      icon: (p) => <CardIcon name="key" size={p.size} />,
      visible: () => may(who(), "admin:ManageMachines"),
    },
  ],
});

export default settingsFeature;
