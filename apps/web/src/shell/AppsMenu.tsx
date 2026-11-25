import { For, Show } from "solid-js";
import { A, useLocation } from "@solidjs/router";
import { t } from "@knime-orchestador/i18n";
import type { AccountApp } from "../auth/api";

/**
 * In the profile menu, where else to go: this installation's applications
 * (RustFS…) and its modules' pages.
 */
export function AppsMenu(props: {
  /** The other applications; `null` when they did not answer. */
  apps: AccountApp[] | null | undefined;
  onClose: () => void;
  onLeave: (e: MouseEvent, message: string) => void;
}) {
  // The one open now (/apps/<id>) is not offered: one is already in it.
  const location = useLocation();
  const shownApps = () =>
    props.apps?.filter((a) => !(a.kind === "app" && a.id && location.pathname === `/apps/${a.id}`));
  // Inside one, Quality Data is one more to go to (the way back).
  const inApp = () => location.pathname.startsWith("/apps/");
  const hasApps = () => inApp() || (shownApps()?.length ?? 0) > 0;
  return (
    <Show when={hasApps()}>
      <>
        <div class="profile-menu__divider" />
        <nav class="profile-menu" aria-label={t("web:shell.apps")}>
          <span class="profile-menu__heading">{t("web:shell.apps")}</span>
          <Show when={inApp()}>
            <A
              class="profile-menu__item profile-menu__app"
              role="menuitem"
              href="/"
              onClick={() => props.onClose()}
            >
              <span class="profile-menu__icon">
                <img class="profile-menu__app-logo" src="/logo.svg" alt="" width="18" height="18" />
              </span>
              <span class="profile-menu__app-text">
                <span class="profile-menu__app-name">{t("web:app.name")}</span>
                <span class="profile-menu__app-detail">{t("web:shell.backToApp")}</span>
              </span>
            </A>
          </Show>
          <Show when={shownApps()?.length ? shownApps() : undefined}>
            {(list) => (
              <For each={list()}>
                {(app) => (
                  <a
                    class="profile-menu__item profile-menu__app"
                    classList={{ "profile-menu__app--current": app.current }}
                    role="menuitem"
                    // This installation's (RustFS…) open inside the app;
                    // a module's page (the user guide…) in this tab: a full
                    // load.
                    href={app.kind === "app" && app.id ? `/apps/${app.id}` : app.url}
                    rel={app.kind === "app" ? undefined : "external"}
                    aria-current={app.current ? "page" : undefined}
                    onClick={(e) => {
                      if (app.kind === "app" || app.kind === "module") return props.onClose();
                      if (!app.current) props.onLeave(e, t("web:shell.opening", { app: app.name }));
                    }}
                  >
                    <Show when={app.icon}>
                      {(icon) => (
                        <span class="profile-menu__icon">
                          <span
                            class="profile-menu__app-icon"
                            style={{ "mask-image": `url("${icon()}")` }}
                            aria-hidden="true"
                          />
                        </span>
                      )}
                    </Show>
                    <span class="profile-menu__app-text">
                      <span class="profile-menu__app-name">{app.name}</span>
                      <span class="profile-menu__app-detail">{app.detail}</span>
                    </span>
                  </a>
                )}
              </For>
            )}
          </Show>
        </nav>
      </>
    </Show>
  );
}
