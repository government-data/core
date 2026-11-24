import { createResource, createSignal, onCleanup, Show, type Component } from "solid-js";
import { Dynamic, Portal } from "solid-js/web";
import { t } from "@knime-orchestador/i18n";
import { useNavigate } from "@solidjs/router";
import { SettingsNavIcon } from "@knime-orchestador/feature-settings";
import { Leaving } from "@knime-orchestador/feature-kit/ui";
import { signedOut } from "@knime-orchestador/feature-kit";
import { useSession } from "../auth/session";
import { accountApps, accountUrl, logout, saveLocale } from "../auth/api";
import { AppsMenu } from "./AppsMenu";
import { LangSwitch } from "./LangSwitch";
import { AccountIcon, MENU_ICON, MenuIcon } from "./MenuIcon";
import { ThemeSwitch } from "./ThemeSwitch";

/** The initial shown in the avatar, derived from the admin's role. */
function avatarInitial(role: string): string {
  return (role[0] ?? "?").toUpperCase();
}

/** Profile menu items, each shown only to the roles listed (without `roles`,
 * to everyone). */
const PROFILE_MENU: {
  icon: Component<{ size?: number }>;
  label: string;
  path: string;
  roles?: string[];
}[] = [
  {
    icon: AccountIcon,
    get label() {
      return t("web:shell.account");
    },
    // Everyone: their email, role, photo and linked accounts.
    path: "/settings/account",
  },
  {
    icon: SettingsNavIcon,
    get label() {
      return t("web:shell.settings");
    },
    // Everyone: «Mi cuenta» is there; the rest is only for admin.
    path: "/settings",
  },
];

/**
 * The profile photo (the one Google or another provider gave on sign-in or
 * linking) or, without it or if it does not load, the initial.
 */
function Avatar(p: { class: string }) {
  const session = useSession();
  const [noPhoto, setNoPhoto] = createSignal(false);
  return (
    <Show
      when={session.avatar && !noPhoto()}
      fallback={
        <span class={p.class} aria-hidden="true">
          {avatarInitial(session.role)}
        </span>
      }
    >
      <img
        class={`${p.class} avatar--photo`}
        src={`/api/auth/me/avatar?v=${session.avatar}`}
        alt=""
        aria-hidden="true"
        referrerpolicy="no-referrer"
        onError={() => setNoPhoto(true)}
      />
    </Show>
  );
}

/**
 * The foot of the profile menu: the theme, the language and signing out.
 * The user guide is a module: its entry comes with the applications.
 */
function MenuFoot(props: { signOut: () => void }) {
  return (
    <>
      <div class="profile-menu__divider" />
      <div class="profile-menu__lang">
        <span class="profile-menu__lang-label">{t("web:shell.theme")}</span>
        <ThemeSwitch />
      </div>
      <div class="profile-menu__divider" />
      <div class="profile-menu__lang">
        <span class="profile-menu__lang-label">{t("web:shell.language")}</span>
        <LangSwitch onSave={saveLocale} />
      </div>

      <div class="profile-menu__divider" />

      <button
        class="profile-menu__item profile-menu__item--danger"
        type="button"
        role="menuitem"
        onClick={() => props.signOut()}
      >
        <span class="profile-menu__icon">
          <MenuIcon name="signOut" />
        </span>
        <span>{t("web:shell.signOut")}</span>
      </button>
    </>
  );
}

/** The top of the profile menu: who is signed in, their account and Settings. */
function ProfileCard(props: { onNavigate: (path: string) => void }) {
  const session = useSession();
  const profileMenu = PROFILE_MENU.filter(
    (item) => !item.roles || item.roles.includes(session.role),
  );
  const displayName = () => session.display_name ?? session.role;
  const email = () => session.email ?? "";
  return (
    <>
      <div class="profile-card">
        <Avatar class="profile-card__avatar" />
        <div class="profile-card__info">
          <span class="profile-card__name">{displayName()}</span>
          <Show when={email()}>
            <span class="profile-card__email">{email()}</span>
          </Show>
          <span class="profile-card__role">{session.role}</span>
        </div>
      </div>

      <Show when={profileMenu.length > 0}>
        <div class="profile-menu__divider" />

        <nav class="profile-menu">
          {profileMenu.map((item) => (
            <button
              class="profile-menu__item"
              type="button"
              role="menuitem"
              onClick={() => props.onNavigate(item.path)}
            >
              <span class="profile-menu__icon">
                <Dynamic component={item.icon} size={MENU_ICON} />
              </span>
              <span>{item.label}</span>
            </button>
          ))}
        </nav>
      </Show>
    </>
  );
}

/**
 * The signed-in person at the top right and, opened, their menu: their
 * account, Settings, the other applications and modules, theme, language and
 * signing out.
 */
export function ProfileMenu() {
  const session = useSession();
  const [profileOpen, setProfileOpen] = createSignal(false);
  const navigate = useNavigate();
  const displayName = () => session.display_name ?? session.role;

  // Sign out server-side, then hard-redirect. The full reload clears every
  // cached query so no authenticated data lingers after logout.
  // With the common account, through its /signout: it closes that session too
  // and goes back to the site.
  const [account] = createResource(() => accountUrl().catch(() => null));
  // The other applications: this installation's (RustFS…) and the account's;
  // `null` when they did not answer (with the account, the menu then offers
  // its own list).
  const [apps] = createResource(() => accountApps().catch(() => null));
  // Going to another application: until it paints (with the sign-in round
  // trip it can take seconds), the wait animation and where to. Opening it in
  // another tab or window does not leave this page.
  const [leaving, setLeaving] = createSignal<string>();
  const leaveTo = (e: MouseEvent, name: string) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey)
      return;
    setLeaving(name);
  };
  // Back from the other app, the page comes out of the cache as it was.
  const onPageShow = (e: PageTransitionEvent) => e.persisted && setLeaving(undefined);
  window.addEventListener("pageshow", onPageShow);
  onCleanup(() => window.removeEventListener("pageshow", onPageShow));
  const signOut = async () => {
    // What the features kept in the tab (a view's reason, 0069) goes first.
    signedOut();
    await logout().catch(() => undefined);
    const base = account() ?? (await accountUrl().catch(() => null));
    window.location.assign(base ? `${base}/signout` : "/login");
  };

  // Close profile popover on click-outside or ESC.
  let profileRef: HTMLDivElement | undefined;

  const handleClickOutside = (e: MouseEvent) => {
    if (profileRef && !profileRef.contains(e.target as Node)) {
      setProfileOpen(false);
    }
  };

  const handleEsc = (e: KeyboardEvent) => {
    if (e.key === "Escape") setProfileOpen(false);
  };

  // Attach/detach listeners based on popover state.
  const startListening = () => {
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEsc);
  };
  const stopListening = () => {
    document.removeEventListener("mousedown", handleClickOutside);
    document.removeEventListener("keydown", handleEsc);
  };
  onCleanup(stopListening);

  const toggleProfile = () => {
    const next = !profileOpen();
    setProfileOpen(next);
    if (next) startListening();
    else stopListening();
  };

  const navigateTo = (path: string) => {
    setProfileOpen(false);
    stopListening();
    navigate(path);
  };
  const closeMenu = () => {
    setProfileOpen(false);
    stopListening();
  };

  return (
    <>
      {/* Profile trigger + popover */}
      <div class="profile-wrapper" ref={profileRef}>
        <button
          class="profile-trigger"
          type="button"
          aria-expanded={profileOpen()}
          aria-haspopup="true"
          onClick={toggleProfile}
        >
          <Avatar class="avatar" />
          <span class="topbar__user-name">{displayName()}</span>
        </button>

        <Show when={profileOpen()}>
          <div class="profile-popover" role="menu">
            <ProfileCard onNavigate={navigateTo} />
            <AppsMenu account={account()} apps={apps()} onClose={closeMenu} onLeave={leaveTo} />
            <MenuFoot signOut={() => void signOut()} />
          </div>
        </Show>
      </div>
      <Show when={leaving()}>
        {(message) => (
          <Portal>
            <Leaving message={message()} />
          </Portal>
        )}
      </Show>
    </>
  );
}
