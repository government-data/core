import { createSignal, For, Show, type ParentProps } from "solid-js";
import { isLang, lang, setLang, t } from "@knime-orchestador/i18n";
import { A } from "@solidjs/router";
import { Dynamic } from "solid-js/web";
// It also brings feature-kit's base styles (.fk-btn) for every view.
import "@knime-orchestador/feature-kit/ui";
import {
  SettingsCardsContext,
  type NavItem,
  type SidebarPanel,
} from "@knime-orchestador/feature-kit";
import { collectNav, collectOverlays, collectPanels, collectSettingsCards } from "./compose";
import { features } from "./registry";
import { useSession } from "../auth/session";
import { ProfileMenu } from "./ProfileMenu";
import { SidebarResizer } from "./SidebarResizer";
import { useSidebarWidth } from "./sidebarWidth";
import { VersionFooter } from "./VersionFooter";

/** Where the collapsed-sidebar preference is remembered (per browser). */
const COLLAPSED_KEY = "orq.nav.collapsed";

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) === "1";
  } catch {
    return false;
  }
}

function saveCollapsed(collapsed: boolean) {
  try {
    localStorage.setItem(COLLAPSED_KEY, collapsed ? "1" : "0");
  } catch {
    // Without storage the choice just lasts until the next reload.
  }
}

/** Whether the account's language was applied in this page (see AdminLayout). */
let accountLocaleApplied = false;

/** The features' entries, each shown while it may be. */
function SidebarNav(props: { items: NavItem[]; onPick: () => void }) {
  return (
    <nav class="sidebar__nav">
      <For each={props.items}>
        {(item) => (
          <Show when={!item.visible || item.visible()}>
            <A
              class="nav-link"
              activeClass="nav-link--active"
              href={item.path}
              end={item.path === "/"}
              onClick={() => props.onPick()}
              data-tip={item.label}
              aria-label={item.label}
            >
              <span class="nav-link__icon" aria-hidden="true">
                {typeof item.icon === "function" ? (
                  <Dynamic component={item.icon} size={20} />
                ) : (
                  item.icon || item.label.slice(0, 1)
                )}
              </span>
              <span class="sidebar__label">{item.label}</span>
            </A>
          </Show>
        )}
      </For>
    </nav>
  );
}

/** The features' panels, under the entries; hidden while the sidebar is minimized. */
function SidebarPanels(props: { panels: SidebarPanel[] }) {
  return (
    <For each={props.panels}>
      {(panel) => (
        <Show when={!panel.visible || panel.visible()}>
          <section class="sidebar__panel" aria-label={panel.label}>
            <Dynamic component={panel.component} />
          </section>
        </Show>
      )}
    </For>
  );
}

/**
 * The responsive admin chrome: a sidebar of feature navigation, a top header
 * showing the signed-in admin with a sign-out control, and the content area
 * where the matched route renders. `props.children` is the active route's view.
 *
 * Rendered only inside `<RequireSession>`, so {@link useSession} always resolves
 * to a real identity here.
 *
 * On narrow viewports the sidebar collapses into an off-canvas drawer toggled
 * from the header; on wide viewports it is a permanent column that a button at
 * its top corner minimizes to icons only (remembered per browser), leaving the room
 * to the content; the labels then show as tooltips.
 */
export function AdminLayout(props: ParentProps) {
  const [navOpen, setNavOpen] = createSignal(false);
  const [collapsed, setCollapsed] = createSignal(readCollapsed());
  const toggleCollapsed = () => {
    const next = !collapsed();
    setCollapsed(next);
    saveCollapsed(next);
  };
  const session = useSession();
  // The language saved in the account wins over the browser's: if it is
  // another, it is applied (without reloading: the texts follow). Once per page:
  // choosing another rebuilds the app, before the account has saved it.
  if (!accountLocaleApplied && isLang(session.locale) && session.locale !== lang()) {
    setLang(session.locale);
  }
  accountLocaleApplied = true;
  const nav = collectNav(features, session.role);
  const overlays = collectOverlays(features, session.role);
  const panels = collectPanels(features, session.role);
  const hasPanel = () => panels.some((p) => !p.visible || p.visible());
  const sidebarWidth = useSidebarWidth();
  const width = () => (hasPanel() && sidebarWidth.width()) || undefined;
  const settingsCards = collectSettingsCards(features, session.role);
  const closeNav = () => setNavOpen(false);

  return (
    <div
      class="admin"
      classList={{
        "admin--nav-open": navOpen(),
        "admin--nav-collapsed": collapsed(),
        "admin--with-panel": hasPanel(),
      }}
      style={width() ? { "--sidebar-width": `${width()}px` } : undefined}
    >
      <aside class="sidebar" aria-label={t("web:shell.primary")}>
        <div class="sidebar__top">
          <button
            class="sidebar__collapse"
            type="button"
            aria-pressed={collapsed()}
            title={collapsed() ? t("web:shell.expand") : t("web:shell.collapse")}
            aria-label={collapsed() ? t("web:shell.expand") : t("web:shell.collapse")}
            onClick={toggleCollapsed}
          >
            {/* Menu lines and a chevron: it turns to point outward when minimized. */}
            <svg viewBox="0 0 20 20" width="18" height="18" aria-hidden="true">
              <path
                d="M3 5h8M3 10h6M3 15h8M16 6.5 12.5 10l3.5 3.5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
        </div>
        <div class="sidebar__brand">
          <img class="sidebar__mark" src="/logo-64.png" alt="" width="22" height="22" />
          <span class="sidebar__label">{t("web:app.name")}</span>
        </div>
        <SidebarNav items={nav} onPick={closeNav} />
        <SidebarPanels panels={panels} />
        <VersionFooter
          api={session.version ? { version: session.version, commit: session.commit } : undefined}
          role={session.role}
        />
        <Show when={hasPanel() && !collapsed()}>
          <SidebarResizer width={width()} onResize={sidebarWidth.set} />
        </Show>
      </aside>

      <button
        class="scrim"
        aria-label={t("web:shell.closeNav")}
        tabindex={navOpen() ? 0 : -1}
        onClick={closeNav}
      />

      <div class="admin__main">
        <header class="topbar">
          <button
            class="topbar__toggle"
            aria-label={t("web:shell.toggleNav")}
            aria-expanded={navOpen()}
            onClick={() => setNavOpen((open) => !open)}
          >
            ☰
          </button>
          <div class="topbar__spacer" />

          <ProfileMenu />
        </header>

        <main class="content">
          <SettingsCardsContext.Provider value={settingsCards}>
            {props.children}
          </SettingsCardsContext.Provider>
        </main>
      </div>
      {/* Feature UI that outlives route changes (toasts, background tasks). */}
      <For each={overlays}>{(overlay) => <Dynamic component={overlay} />}</For>
    </div>
  );
}
