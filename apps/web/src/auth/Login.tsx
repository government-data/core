/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { createEffect, createSignal, For, Show, type Component } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { useMutation, useQuery } from "@tanstack/solid-query";
import { ApiError } from "@knime-orchestador/query";
import { authKeys, listProviders, login, oauthStartUrl, providerName, captchaSiteKey } from "./api";
import "./login.css";
import { LangSwitch } from "../shell/LangSwitch";
import { needsCaptcha, Turnstile } from "./Turnstile";
import { ACCOUNT_HOME, accountSite, goInAccount, isAccount } from "./account";
import { t } from "@knime-orchestador/i18n";

/** Turn a failed login into a message that never reveals whether the account
 * exists — only "these credentials did not work", "you are being throttled",
 * or "the server side failed" (so an outage is never blamed on the password). */
function loginErrorMessage(error: unknown): string {
  if (needsCaptcha(error)) {
    return t("web:common.errors.captcha");
  }
  if (error instanceof ApiError) {
    if (error.status === 429) {
      return t("web:common.errors.tooMany");
    }
    if (error.status === 503) {
      return t("web:common.errors.directory");
    }
    if (error.status >= 500) {
      return t("web:common.errors.server");
    }
  }
  return t("web:login.invalid");
}

/** The provider's mark, for the ones we know (Google's «G», in its colors). */
const ProviderIcon: Component<{ id: string }> = (props) => (
  <Show when={props.id === "google"}>
    <svg class="login__provider-icon" viewBox="0 0 48 48" aria-hidden="true">
      <path
        fill="#EA4335"
        d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"
      />
      <path
        fill="#4285F4"
        d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"
      />
      <path
        fill="#FBBC05"
        d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"
      />
      <path
        fill="#34A853"
        d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"
      />
    </svg>
  </Show>
);

/** Format a provider id (`google`) into a button label (`Continue with Google`). */
function providerLabel(id: string): string {
  return t("web:login.continueWith", { provider: providerName(id) });
}

/**
 * The sign-in screen. Rendered outside the admin chrome (see the shell router):
 * a password form plus a button per configured OAuth provider. On success it
 * returns to the route the user was sent from (`?next=`), defaulting to the
 * dashboard. Loading and error states come straight off the TanStack Query
 * mutation.
 */
export const Login: Component = () => {
  const [params] = useSearchParams<{
    next?: string;
    error?: string;
    reset?: string;
    activated?: string;
  }>();

  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");

  const inAccount = isAccount();
  const next = () => params.next ?? (inAccount ? ACCOUNT_HOME : "/");
  // Always a fresh page, not a router push: the documentation (/docs/), another
  // application's authorization (/api/oauth/authorize) and the apps behind
  // Traefik's gate are not this SPA, and in the app what this page asked
  // without a session (a 401 kept by a feature's module) would otherwise stay
  // and hang it on its skeleton until a reload.
  const goNext = () => {
    const to = next();
    // In the common account nothing that follows belongs to this app.
    if (inAccount) return goInAccount(to);
    // Only a path of this site: the query can bring a URL (redirect_uri).
    const path = to.split("?", 1)[0];
    const local = to.startsWith("/") && !path.includes("//") && !path.includes("\\");
    window.location.replace(local ? to : "/");
  };

  // In the common account, with the session already open there is no need to sign in again.
  // Without the API client: its 401 would send to sign in, which is this page.
  const hasSession = useQuery(() => ({
    queryKey: authKeys.list({ scope: "account-session" }),
    queryFn: async () =>
      (await fetch("/api/auth/me", { credentials: "same-origin" }).catch(() => null))?.ok === true,
    enabled: inAccount,
    retry: false,
  }));
  createEffect(() => {
    if (hasSession.data) goNext();
  });

  const providers = useQuery(() => ({
    queryKey: authKeys.list({ scope: "oauth-providers" }),
    queryFn: listProviders,
    staleTime: 5 * 60 * 1000,
  }));

  // After several failed attempts, the API asks for the captcha (if it uses one).
  const [captcha, setCaptcha] = createSignal<string | null>(null);
  const [token, setToken] = createSignal<string | null>(null);
  let resetCaptcha = () => {};
  const signIn = useMutation(() => ({
    mutationFn: () => login(email(), password(), token()),
    onSuccess: goNext,
    onError: async (error) => {
      if (needsCaptcha(error) && !captcha()) setCaptcha(await captchaSiteKey());
      resetCaptcha();
    },
  }));

  const onSubmit = (e: Event) => {
    e.preventDefault();
    if (!signIn.isPending) {
      signIn.mutate();
    }
  };

  return (
    <main class="login">
      <form class="login__card" onSubmit={onSubmit}>
        <div class="login__brand">
          <img class="login__mark" src="/logo.svg" alt={t("web:app.name")} width="56" height="56" />
        </div>
        <h1 class="login__title">{t("web:login.title")}</h1>

        <Show when={params.reset === "1"}>
          <p class="login__notice" role="status">
            {t("web:login.passwordChanged")}
          </p>
        </Show>
        <Show when={params.activated === "1"}>
          <p class="login__notice" role="status">
            {t("web:login.activated")}
          </p>
        </Show>
        <Show when={params.error === "oauth"}>
          <p class="login__alert" role="alert">
            {t("web:login.ssoFailed")}
          </p>
        </Show>
        <Show when={signIn.isError}>
          <p class="login__alert" role="alert">
            {loginErrorMessage(signIn.error)}
          </p>
        </Show>

        <label class="login__field">
          <span>{t("web:common.email")}</span>
          <input
            type="email"
            name="email"
            autocomplete="username"
            required
            value={email()}
            onInput={(e) => setEmail(e.currentTarget.value)}
          />
        </label>
        <label class="login__field">
          <span class="login__label-row">
            {t("web:common.password")}
            <A
              class="login__link login__link--inline"
              href={`/forgot-password${email().trim() ? `?email=${encodeURIComponent(email().trim())}` : ""}`}
            >
              {t("web:login.forgot")}
            </A>
          </span>
          <input
            type="password"
            name="password"
            autocomplete="current-password"
            required
            value={password()}
            onInput={(e) => setPassword(e.currentTarget.value)}
          />
        </label>

        <Show when={captcha()}>
          {(siteKey) => (
            <Turnstile
              siteKey={siteKey()}
              onToken={setToken}
              bindReset={(fn) => (resetCaptcha = fn)}
            />
          )}
        </Show>
        <button
          class="login__submit"
          type="submit"
          disabled={signIn.isPending || (!!captcha() && !token())}
        >
          {signIn.isPending ? t("web:login.submitting") : t("web:login.submit")}
        </button>

        <Show when={(providers.data ?? []).length > 0}>
          <div class="login__divider">
            <span>{t("web:common.or")}</span>
          </div>
          <div class="login__providers">
            <For each={providers.data}>
              {(id) => (
                <a class="login__provider" href={oauthStartUrl(id, params.next)} rel="external">
                  <ProviderIcon id={id} />
                  {providerLabel(id)}
                </a>
              )}
            </For>
          </div>
        </Show>

        {/* In the common account: back to the product's site. */}
        <Show when={inAccount}>
          <p class="login__back">
            <a class="login__link" href={accountSite()}>
              ← {t("web:login.backHome")}
            </a>
          </p>
        </Show>
      </form>
      <LangSwitch class="lang-switch lang-switch--login" />
    </main>
  );
};
