import { createSignal } from "solid-js";
import { t } from "@knime-orchestador/i18n";
import { diagnostics, shortCommit, versionLabel, type Build } from "./buildInfo";

/** The UI running in this tab (see vite.config.ts). */
const WEB_BUILD: Build = { version: __APP_VERSION__, commit: __APP_COMMIT__ };

/**
 * At the foot of the menu: the version (UI and API, warning if they do not
 * match) and, on click, the diagnostics copied to paste into a report.
 */
export function VersionFooter(props: { api?: Build; role: string }) {
  const [copied, setCopied] = createSignal(false);
  const label = () => versionLabel(WEB_BUILD, props.api);
  const title = () =>
    [
      t("web:shell.version.web", {
        version: WEB_BUILD.version,
        commit: shortCommit(WEB_BUILD.commit),
      }).trim(),
      props.api
        ? t("web:shell.version.api", {
            version: props.api.version,
            commit: shortCommit(props.api.commit),
          }).trim()
        : "",
      label().mismatch ? t("web:shell.version.mismatch") : "",
      t("web:shell.version.copyHint"),
    ]
      .filter(Boolean)
      .join("\n");
  const copy = () => {
    const text = diagnostics(WEB_BUILD, props.api, {
      host: window.location.host,
      userAgent: navigator.userAgent,
      now: new Date(),
      role: props.role,
    });
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    });
  };
  return (
    <button
      type="button"
      class="sidebar__version"
      classList={{ "sidebar__version--mismatch": label().mismatch }}
      title={title()}
      aria-label={t("web:shell.version.aria", { version: label().text })}
      onClick={copy}
    >
      <span class="sidebar__label">{copied() ? t("web:shell.version.copied") : label().text}</span>
      <span class="sidebar__version-dot" aria-hidden="true" />
    </button>
  );
}
