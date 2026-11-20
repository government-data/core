/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { createSignal, For, Match, Show, Switch, type Component } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { useMutation, useQuery } from "@tanstack/solid-query";
import { ApiError } from "@knime-orchestador/query";
import { authKeys, checkPasswordReset, confirmPasswordReset, type PasswordPolicy } from "./api";
import "./login.css";
import { t } from "@knime-orchestador/i18n";

/**
 * The token from the emailed link (`/reset-password#<token>`). It travels in
 * the fragment so it never reaches a server log; once read it is dropped from
 * the address bar, so it does not linger in the history either.
 */
function takeToken(): string {
  const token = decodeURIComponent(window.location.hash.replace(/^#/, "")).trim();
  if (token) {
    window.history.replaceState(null, "", window.location.pathname);
  }
  return token;
}

/** The rules of the policy, each with whether `password` meets it. */
function rules(policy: PasswordPolicy, password: string): { label: string; ok: boolean }[] {
  const out = [
    {
      label: t("web:reset.rule.length", { count: policy.min_length }),
      ok: [...password].length >= policy.min_length,
    },
  ];
  if (policy.require_uppercase) {
    out.push({ label: t("web:reset.rule.upper"), ok: /[A-Z]/.test(password) });
  }
  if (policy.require_lowercase) {
    out.push({ label: t("web:reset.rule.lower"), ok: /[a-z]/.test(password) });
  }
  if (policy.require_digit) {
    out.push({ label: t("web:reset.rule.digit"), ok: /[0-9]/.test(password) });
  }
  if (policy.require_symbol) {
    out.push({ label: t("web:reset.rule.symbol"), ok: /[^\p{L}\p{N}]/u.test(password) });
  }
  return out;
}

function confirmErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 410) {
      return t("web:reset.invalidLink");
    }
    if (error.status === 422) {
      const field = (error.body as { fields?: Record<string, string> } | undefined)?.fields
        ?.password;
      return field ? t("web:reset.rejected", { reason: field }) : t("web:reset.rules");
    }
    if (error.status === 429) {
      return t("web:common.errors.tooMany");
    }
  }
  return t("web:common.errors.server");
}

/**
 * The form the emailed link opens: checks the link first (so an expired one
 * says so before anything is typed), then asks for the new password twice and
 * shows the policy as it is met. On success every session of the account has
 * ended, so it sends the person to sign in again.
 *
 * With `activation`, it is the form a new account's link opens (`/activate`):
 * the same token and endpoints, with a welcome instead of "forgot it?".
 */
const PasswordForm: Component<{ activation?: boolean }> = (props) => {
  const navigate = useNavigate();
  const token = takeToken();
  // The texts that differ between choosing a first password and a new one.
  const say = (key: "title" | "save" | "hint" | "expired" | "newLink") =>
    t(props.activation ? `web:activate.${key}` : `web:reset.${key}`);

  const [password, setPassword] = createSignal("");
  const [repeat, setRepeat] = createSignal("");

  const check = useQuery(() => ({
    queryKey: authKeys.detail(`password-reset:${token}`),
    queryFn: () => checkPasswordReset(token),
    enabled: token !== "",
    retry: false,
    staleTime: Infinity,
  }));

  const save = useMutation(() => ({
    mutationFn: () => confirmPasswordReset(token, password()),
    onSuccess: () =>
      navigate(props.activation ? "/login?activated=1" : "/login?reset=1", { replace: true }),
  }));

  const valid = () => token !== "" && check.data?.valid === true;
  const meetsPolicy = () => {
    const policy = check.data?.policy;
    return policy ? rules(policy, password()).every((r) => r.ok) : false;
  };
  const matches = () => password() === repeat();
  const linkGone = () => save.error instanceof ApiError && save.error.status === 410;

  const onSubmit = (e: Event) => {
    e.preventDefault();
    if (!save.isPending && meetsPolicy() && matches()) {
      save.mutate();
    }
  };

  const expired = (
    <>
      <p class="login__alert" role="alert">
        {say("expired")}
      </p>
      <A class="login__submit login__submit--link" href="/forgot-password">
        {say("newLink")}
      </A>
    </>
  );

  return (
    <main class="login">
      <form class="login__card" onSubmit={onSubmit}>
        <div class="login__brand">
          <img class="login__mark" src="/logo.svg" alt={t("web:app.name")} width="56" height="56" />
        </div>
        <h1 class="login__title">{say("title")}</h1>

        <Switch>
          <Match when={token !== "" && check.isPending}>
            <p class="login__hint">{t("web:reset.checking")}</p>
          </Match>
          <Match when={token !== "" && check.isError}>
            <p class="login__alert" role="alert">
              {t("web:common.errors.server")}
            </p>
          </Match>
          <Match when={!valid() || linkGone()}>{expired}</Match>
          <Match when={check.data?.policy}>
            {(policy) => (
              <>
                <Show when={save.isError}>
                  <p class="login__alert" role="alert">
                    {confirmErrorMessage(save.error)}
                  </p>
                </Show>
                <label class="login__field">
                  <span>{t("web:reset.newPassword")}</span>
                  <input
                    type="password"
                    name="new-password"
                    autocomplete="new-password"
                    required
                    value={password()}
                    onInput={(e) => setPassword(e.currentTarget.value)}
                  />
                </label>
                <ul class="login__rules">
                  <For each={rules(policy(), password())}>
                    {(rule) => (
                      <li classList={{ "login__rule--ok": rule.ok }}>
                        {rule.ok ? "✓" : "•"} {rule.label}
                      </li>
                    )}
                  </For>
                </ul>
                <label class="login__field">
                  <span>{t("web:reset.repeat")}</span>
                  <input
                    type="password"
                    name="repeat-password"
                    autocomplete="new-password"
                    required
                    value={repeat()}
                    onInput={(e) => setRepeat(e.currentTarget.value)}
                  />
                </label>
                <Show when={repeat() !== "" && !matches()}>
                  <p class="login__field-error">{t("web:reset.mismatch")}</p>
                </Show>
                <button
                  class="login__submit"
                  type="submit"
                  disabled={save.isPending || !meetsPolicy() || !matches()}
                >
                  {save.isPending ? t("web:reset.saving") : say("save")}
                </button>
                <p class="login__hint">{say("hint")}</p>
              </>
            )}
          </Match>
        </Switch>

        <A class="login__link" href="/login">
          {t("web:common.backToLogin")}
        </A>
      </form>
    </main>
  );
};

/** `/reset-password#<token>`: a new password, from "forgot your password?". */
export const ResetPassword: Component = () => <PasswordForm />;

/** `/activate#<token>`: a new account's first password. */
export const ActivateAccount: Component = () => <PasswordForm activation />;
