/* eslint-disable max-lines-per-function -- TODO(0034): split */
import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { ConfirmDialog } from "@knime-orchestador/feature-kit/ui";
import { PROFILE_CHANGED, useSession } from "@knime-orchestador/feature-kit";
import { accountApi, providerName, SettingsApiError, type LinkedIdentity } from "./api";
import { NoticeBox, SectionHeader, useAction } from "./common";
import { t } from "./i18n";

/** The largest photo the server keeps. */
const MAX_PHOTO_BYTES = 256 * 1024;

const roleLabel = (role: string) =>
  role === "admin" || role === "user" ? t(`settings:users.role.${role}`) : role;

/**
 * The person's profile: email, name, role and their photo, which they upload
 * or remove (CUENTA-01/02). The shell's avatar follows.
 */
const Profile: Component = () => {
  const session = useSession();
  const photo = useAction();
  let input: HTMLInputElement | undefined;
  const changed = () => window.dispatchEvent(new Event(PROFILE_CHANGED));
  const upload = (file: File | undefined) => {
    if (!file) return;
    if (file.size > MAX_PHOTO_BYTES) {
      void photo.run(async () => {
        throw new SettingsApiError(
          413,
          t("settings:account.photoTooBig", { kb: Math.ceil(file.size / 1024) }),
        );
      });
      return;
    }
    void photo.run(async () => {
      await accountApi.uploadPhoto(file);
      changed();
      return t("settings:account.photoSaved");
    });
  };
  const remove = () =>
    void photo.run(async () => {
      await accountApi.removePhoto();
      changed();
      return t("settings:account.photoRemoved");
    });
  return (
    <div class="card st-profile">
      <div class="st-profile__photo">
        <Show
          when={session.avatar}
          fallback={
            <span class="st-profile__initial" aria-hidden="true">
              {(session.display_name || session.email || "?").slice(0, 1).toUpperCase()}
            </span>
          }
        >
          <img
            class="st-profile__img"
            src={`/api/auth/me/avatar?v=${session.avatar}`}
            alt={t("settings:account.photo")}
          />
        </Show>
      </div>
      <div class="st-profile__body">
        <dl class="st-profile__facts">
          <dt>{t("settings:account.email")}</dt>
          <dd>{session.email || "—"}</dd>
          <dt>{t("settings:account.name")}</dt>
          <dd>{session.display_name || t("settings:account.noName")}</dd>
          <dt>{t("settings:account.role")}</dt>
          <dd>{roleLabel(session.role)}</dd>
        </dl>
        <NoticeBox notice={photo.notice()} />
        <div class="st-profile__actions">
          <input
            ref={input}
            type="file"
            accept="image/jpeg,image/png,image/webp,image/gif"
            class="sr-only"
            aria-label={t("settings:account.uploadPhoto")}
            onChange={(e) => {
              upload(e.currentTarget.files?.[0]);
              e.currentTarget.value = "";
            }}
          />
          <button
            type="button"
            class="fk-btn fk-btn--sm"
            disabled={photo.busy()}
            onClick={() => input?.click()}
          >
            {session.avatar ? t("settings:account.changePhoto") : t("settings:account.uploadPhoto")}
          </button>
          <Show when={session.avatar}>
            <button
              type="button"
              class="fk-btn fk-btn--sm fk-btn--danger"
              disabled={photo.busy()}
              onClick={remove}
            >
              {t("settings:account.removePhoto")}
            </button>
          </Show>
        </div>
        <p class="table__muted st-profile__hint">{t("settings:account.photoHint")}</p>
      </div>
    </div>
  );
};

/**
 * «Mi cuenta»: the profile and the Google, Entra ID or other OIDC accounts
 * each person signs in to theirs with, whatever email they bring. Linking is
 * going to the provider and back (`?linked=` or `?error=`); unlinking, one
 * click. Any role sees it; without configured providers there is nothing to
 * link.
 */
export const AccountPage: Component = () => {
  const [params, setParams] = useSearchParams<{ linked?: string; error?: string }>();
  const [providers] = createResource(accountApi.providers);
  // Without OAuth the route does not exist: there is nothing to list.
  const [identities, { refetch }] = createResource(
    () => ((providers()?.length ?? 0) > 0 ? true : undefined),
    () => accountApi.identities(),
  );
  const [unlinking, setUnlinking] = createSignal<LinkedIdentity>();
  const unlink = useAction();

  // The configured providers and, in case one was removed, those of accounts
  // already linked (so they can be unlinked).
  const rows = () => {
    const ids = new Set(providers() ?? []);
    for (const i of identities() ?? []) ids.add(i.provider);
    return [...ids].sort();
  };
  const linkedTo = (provider: string) =>
    (identities() ?? []).filter((i) => i.provider === provider);
  const configured = (provider: string) => (providers() ?? []).includes(provider);

  const returned = () => {
    if (params.linked) {
      return {
        kind: "ok" as const,
        text: t("settings:account.linked", { provider: providerName(params.linked) }),
      };
    }
    if (params.error === "taken") {
      return { kind: "error" as const, text: t("settings:account.errors.taken") };
    }
    if (params.error) return { kind: "error" as const, text: t("settings:account.errors.failed") };
    return undefined;
  };

  const confirmUnlink = () => {
    const identity = unlinking();
    if (!identity) return;
    void unlink
      .run(async () => {
        await accountApi.unlink(identity);
        setParams({ linked: undefined, error: undefined });
        await refetch();
      })
      .finally(() => setUnlinking(undefined));
  };

  return (
    <section class="feature">
      <SectionHeader
        title={t("settings:account.title")}
        subtitle={t("settings:account.subtitle")}
      />
      <Profile />
      <h2 class="st-section-title">{t("settings:account.linkedTitle")}</h2>
      <p class="table__muted">{t("settings:account.linkedSubtitle")}</p>
      <NoticeBox notice={returned()} />
      <NoticeBox notice={unlink.notice()} />

      <Show when={identities.error}>
        <p class="st-notice st-notice--error">{t("settings:account.errors.load")}</p>
      </Show>

      <Show
        when={!providers.loading}
        fallback={<p class="card st-empty">{t("settings:account.loading")}</p>}
      >
        <Show
          when={rows().length > 0}
          fallback={<p class="card st-empty">{t("settings:account.noProviders")}</p>}
        >
          <div class="card">
            <table class="table">
              <thead>
                <tr>
                  <th>{t("settings:account.provider")}</th>
                  <th>{t("settings:account.account")}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                <For each={rows()}>
                  {(provider) => (
                    <tr>
                      <td>{providerName(provider)}</td>
                      <td>
                        <Show
                          when={linkedTo(provider).length > 0}
                          fallback={
                            <span class="table__muted">{t("settings:account.notLinked")}</span>
                          }
                        >
                          <ul class="st-linked">
                            <For each={linkedTo(provider)}>
                              {(identity) => (
                                <li class="st-linked__item">
                                  <span>{identity.email ?? t("settings:account.noEmail")}</span>
                                  <button
                                    type="button"
                                    class="fk-btn fk-btn--sm fk-btn--danger"
                                    onClick={() => setUnlinking(identity)}
                                  >
                                    {t("settings:account.unlink")}
                                  </button>
                                </li>
                              )}
                            </For>
                          </ul>
                        </Show>
                      </td>
                      <td class="st-linked__actions">
                        <Show when={configured(provider)}>
                          <a
                            class="fk-btn fk-btn--sm"
                            href={accountApi.linkUrl(provider)}
                            rel="external"
                          >
                            {linkedTo(provider).length > 0
                              ? t("settings:account.linkAnother")
                              : t("settings:account.link")}
                          </a>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </Show>
      </Show>

      <ConfirmDialog
        open={unlinking() !== undefined}
        title={t("settings:account.confirmTitle")}
        message={t("settings:account.confirmBody", {
          provider: providerName(unlinking()?.provider ?? ""),
          account: unlinking()?.email ?? t("settings:account.noEmail"),
        })}
        confirmLabel={t("settings:account.unlink")}
        pending={unlink.busy()}
        onCancel={() => setUnlinking(undefined)}
        onConfirm={confirmUnlink}
      />
    </section>
  );
};
