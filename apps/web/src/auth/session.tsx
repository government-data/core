import { createEffect, onCleanup, Show, type JSX, type ParentProps } from "solid-js";
import { Spiral } from "@knime-orchestador/feature-kit/ui";
import { useQuery } from "@tanstack/solid-query";
import { isUnauthorized } from "@knime-orchestador/query";
import {
  PROFILE_CHANGED,
  SessionContext,
  sessionExpired,
  useSession,
  useHasRole,
} from "@knime-orchestador/feature-kit";
import { getMe, meKey, type Me } from "./api";
import { t } from "@knime-orchestador/i18n";

// Re-export so existing shell imports keep working.
export { useSession, useHasRole };
export type { Me };

/**
 * Gate a subtree behind a valid session (AC: admin routes inaccessible without
 * one). It resolves `/auth/me` before rendering: while pending it shows a
 * loader; on success it provides the session and renders its children; on a
 * 401 the global interceptor has already hard-redirected to `/login?next=…`
 * (see `redirectToLoginOnUnauthorized`), so this only ever renders a brief
 * "redirecting" state for that case.
 *
 * Any other failure (the server restarting answers 502 for a few seconds) is
 * retried for about half a minute; if it still fails, it goes to sign in as
 * well, where the form says whether the server answers.
 */
export function RequireSession(props: ParentProps): JSX.Element {
  const me = useQuery(() => ({
    queryKey: meKey(),
    queryFn: getMe,
    // A 401 is a definitive "no session" — don't retry it; fail fast so the
    // global handler redirects. Anything else is retried with backoff (1, 2,
    // 4, 8, 8 s), which covers a restart. staleTime keeps the check from
    // refetching on every navigation within a session.
    retry: (failures, error) => !isUnauthorized(error) && failures < 5,
    retryDelay: (attempt) => Math.min(1000 * 2 ** attempt, 8000),
    staleTime: 60_000,
  }));
  // Out of retries: to sign in, coming back here afterwards.
  createEffect(() => {
    if (me.isError && !isUnauthorized(me.error)) sessionExpired();
  });
  // A feature changed the profile (the photo): the session is read again.
  const changed = () => void me.refetch();
  window.addEventListener(PROFILE_CHANGED, changed);
  onCleanup(() => window.removeEventListener(PROFILE_CHANGED, changed));

  return (
    <Show when={!me.isPending} fallback={<SessionSkeleton label={t("web:session.loading")} />}>
      <Show when={me.data} fallback={<SessionSplash message={t("web:session.redirecting")} />}>
        {(data) => (
          <SessionContext.Provider value={() => data()}>{props.children}</SessionContext.Provider>
        )}
      </Show>
    </Show>
  );
}

/**
 * While the session resolves: the shape of the app (sidebar, top bar,
 * content) in grey, instead of a message. The label is for screen readers.
 */
function SessionSkeleton(props: { label: string }): JSX.Element {
  return (
    <div class="session-skeleton" aria-busy="true" role="status">
      <span class="sr-only">{props.label}</span>
      <div class="session-skeleton__side" aria-hidden="true">
        <span class="session-skeleton__bar session-skeleton__bar--brand" />
        <span class="session-skeleton__bar" />
        <span class="session-skeleton__bar" />
        <span class="session-skeleton__bar" />
      </div>
      <div class="session-skeleton__main" aria-hidden="true">
        <div class="session-skeleton__top">
          <span class="session-skeleton__avatar" />
        </div>
        <div class="session-skeleton__content">
          <span class="session-skeleton__bar session-skeleton__bar--title" />
          <span class="session-skeleton__bar session-skeleton__bar--wide" />
          <span class="session-skeleton__bar session-skeleton__bar--wide" />
          <span class="session-skeleton__bar session-skeleton__bar--wide" />
        </div>
      </div>
    </div>
  );
}

/** A minimal full-viewport status screen used while the session resolves. */
function SessionSplash(props: { message: string }): JSX.Element {
  return (
    <div class="session-splash">
      <Spiral />
      <p role="status">{props.message}</p>
    </div>
  );
}
