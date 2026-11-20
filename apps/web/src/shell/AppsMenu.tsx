import { For, Show } from "solid-js";
import { A, useLocation } from "@solidjs/router";
import { lang, t } from "@knime-orchestador/i18n";
import type { AccountApp } from "../auth/api";
import { MenuIcon } from "./MenuIcon";

/**
 * An account application in the interface language: the account sends the
 * names in Spanish, with their `kind`. The billing also gets `?lang=`, so the
 * account's pages follow the language chosen here.
 */
function inLang(app: AccountApp): AccountApp {
  switch (app.kind) {
    case "management":
      return {
        ...app,
        name: t("web:shell.app.gestion"),
        detail: t("web:shell.app.gestionDetail"),
      };
    case "billing": {
      const url = new URL(app.url, window.location.href);
      url.searchParams.set("lang", lang());
      return {
        ...app,
        name: t("web:shell.app.billing"),
        detail: t("web:shell.app.billingDetail"),
        url: url.toString(),
      };
    }
    case "trial":
      return { ...app, name: t("web:shell.app.trial") };
    default:
      return app;
  }
}

/**
 * In the profile menu (with the account, without other applications, it
 * offers its own list), where else to go: this installation's applications
 * (RustFS…) and the account's; without them, the account's own chooser.
 */
export function AppsMenu(props: {
  account: string | null | undefined;
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
  const hasApps = () => inApp() || !!props.account || (shownApps()?.length ?? 0) > 0;
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
          <Show
            when={shownApps()?.length ? shownApps() : undefined}
            fallback={
              <Show when={props.account}>
                <a
                  class="profile-menu__item"
                  role="menuitem"
                  href={`${props.account}/home?choose&lang=${lang()}`}
                  onClick={(e) => props.onLeave(e, t("web:shell.openingAccount"))}
                >
                  <span class="profile-menu__icon">
                    <MenuIcon name="switch" />
                  </span>
                  <span>{t("web:shell.switchApp")}</span>
                </a>
              </Show>
            }
          >
            {(list) => (
              <For each={list().map(inLang)}>
                {(app) => (
                  <a
                    class="profile-menu__item profile-menu__app"
                    classList={{ "profile-menu__app--current": app.current }}
                    role="menuitem"
                    // This installation's (RustFS…) open inside the app;
                    // the account's are other sites: a full load.
                    href={app.kind === "app" && app.id ? `/apps/${app.id}` : app.url}
                    rel={app.kind === "app" ? undefined : "external"}
                    aria-current={app.current ? "page" : undefined}
                    onClick={(e) => {
                      if (app.kind === "app") return props.onClose();
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
