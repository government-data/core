/**
 * The system sections on Settings' home: their card and their state in one
 * line.
 */
import type { AllSettings } from "./api";
import type { CardIconName } from "./CardIcon";
import { t } from "./i18n";

export interface SettingsSection {
  path: string;
  icon: CardIconName;
  label: string;
  description: string;
  status: (s: AllSettings) => { on: boolean; text: string };
}

const fromEnv = (kind: string) => (kind === "environment" ? t("settings:home.fromEnv") : "");

export const SECTIONS: SettingsSection[] = [
  {
    path: "/settings/ldap",
    icon: "directory",
    get label() {
      return t("settings:home.ldap.label");
    },
    get description() {
      return t("settings:home.ldap.desc");
    },
    status: (s) => ({
      on: s.ldap.value.enabled,
      text:
        (s.ldap.value.enabled ? t("settings:home.on") : t("settings:home.off")) +
        fromEnv(s.ldap.source.kind),
    }),
  },
  {
    path: "/settings/mail",
    icon: "mail",
    get label() {
      return t("settings:home.mail.label");
    },
    get description() {
      return t("settings:home.mail.desc");
    },
    // There is no other mail, so without it no notice goes out.
    status: (s) =>
      s.mail.value.enabled
        ? {
            on: true,
            text:
              s.mail.value.transport === "graph"
                ? t("settings:home.onGraph")
                : t("settings:home.onSmtp"),
          }
        : { on: false, text: t("settings:home.mail.required") },
  },
  {
    path: "/settings/oauth",
    icon: "shield",
    get label() {
      return t("settings:home.oauth.label");
    },
    get description() {
      return t("settings:home.oauth.desc");
    },
    status: (s) => {
      const on = s.oauth.value.providers.filter((p) => p.enabled);
      return {
        on: on.length > 0,
        text:
          (on.length > 0 ? on.map((p) => p.id).join(", ") : t("settings:home.noProviders")) +
          fromEnv(s.oauth.source.kind),
      };
    },
  },
  {
    path: "/settings/platform",
    icon: "clock",
    get label() {
      return t("settings:home.platform.label");
    },
    get description() {
      return t("settings:home.platform.desc");
    },
    status: (s) => ({ on: true, text: s.platform.value.timezone }),
  },
];
