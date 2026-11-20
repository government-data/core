import { createResource, createSignal, Show, type Component } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { useMutation } from "@tanstack/solid-query";
import { ApiError } from "@knime-orchestador/query";
import { captchaSiteKey, requestPasswordReset } from "./api";
import { needsCaptcha, Turnstile } from "./Turnstile";
import "./login.css";
import { t } from "@knime-orchestador/i18n";

function requestErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 429) {
      return t("web:common.errors.tooMany");
    }
    if (error.status === 503) {
      return t("web:forgot.noMail");
    }
    if (error.status === 422) {
      return t("web:forgot.invalidEmail");
    }
    if (needsCaptcha(error)) {
      return t("web:common.errors.captcha");
    }
  }
  return t("web:common.errors.server");
}

/**
 * "Forgot your password?": asks for the email and sends a reset link. The
 * confirmation reads the same whether or not the account exists, so the
 * screen never reveals which emails are registered.
 */
export const ForgotPassword: Component = () => {
  const [params] = useSearchParams<{ email?: string }>();
  const [email, setEmail] = createSignal(params.email ?? "");
  // Each request sends an email: with a captcha, if the installation uses one.
  const [siteKey] = createResource(captchaSiteKey);
  const [token, setToken] = createSignal<string | null>(null);
  let resetCaptcha = () => {};

  const send = useMutation(() => ({
    mutationFn: () => requestPasswordReset(email(), token()),
    // The token works once: after a failed attempt, another one.
    onError: () => resetCaptcha(),
  }));

  const onSubmit = (e: Event) => {
    e.preventDefault();
    if (!send.isPending) {
      send.mutate();
    }
  };

  return (
    <main class="login">
      <form class="login__card" onSubmit={onSubmit}>
        <div class="login__brand">
          <img class="login__mark" src="/logo.svg" alt={t("web:app.name")} width="56" height="56" />
        </div>
        <h1 class="login__title">{t("web:forgot.title")}</h1>

        <Show
          when={send.isSuccess}
          fallback={
            <>
              <p class="login__hint">{t("web:forgot.intro")}</p>
              <Show when={send.isError}>
                <p class="login__alert" role="alert">
                  {requestErrorMessage(send.error)}
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
              <Show when={siteKey()}>
                {(key) => (
                  <Turnstile
                    siteKey={key()}
                    onToken={setToken}
                    bindReset={(fn) => (resetCaptcha = fn)}
                  />
                )}
              </Show>
              <button
                class="login__submit"
                type="submit"
                disabled={send.isPending || (!!siteKey() && !token())}
              >
                {send.isPending ? t("web:forgot.sending") : t("web:forgot.send")}
              </button>
            </>
          }
        >
          <p class="login__notice" role="status">
            {t("web:forgot.sent", {
              email: email().trim(),
              minutes: send.data?.expires_in_minutes,
            })}
          </p>
          <p class="login__hint">{t("web:forgot.directoryHint")}</p>
        </Show>

        <A class="login__link" href="/login">
          {t("web:common.backToLogin")}
        </A>
      </form>
    </main>
  );
};
