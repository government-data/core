/* eslint-disable max-lines-per-function -- TODO(0034): split */
import { createSignal, For, onMount, Show, type Component } from "solid-js";
import { Spiral } from "@knime-orchestador/feature-kit/ui";
import { useMutation } from "@tanstack/solid-query";
import { ApiError } from "@knime-orchestador/query";
import { t } from "@knime-orchestador/i18n";
import { captchaSiteKey } from "./api";
import { needsCaptcha, Turnstile } from "./Turnstile";
import { LangSwitch } from "../shell/LangSwitch";
import "./login.css";

/** The online edition's common entrance: `entrar.<domain>`. */
export const CENTRAL_HOST = "entrar.";

/** Whether this page is the common entrance (and not a company's subdomain). */
export function isCentralHost(hostname = window.location.hostname): boolean {
  return hostname.startsWith(CENTRAL_HOST);
}

/** A company one can sign in to, with its pass. */
interface Company {
  slug: string;
  name: string;
  url: string;
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const data: unknown = await res.json().catch(() => undefined);
  if (!res.ok) throw new ApiError(res.status, res.statusText, data);
  return data as T;
}

/** The same rule as the sign-in: it never says whether the email exists. */
function errorMessage(error: unknown): string {
  if (needsCaptcha(error)) return t("web:common.errors.captcha");
  if (error instanceof ApiError) {
    if (error.status === 429) return t("web:common.errors.tooMany");
    if (error.status === 503) return t("web:common.errors.directory");
    if (error.status >= 500) return t("web:common.errors.server");
  }
  return t("web:login.invalid");
}

/**
 * `entrar.<domain>`: the email and the password, without the company's
 * address. With one company it goes straight in; with several, one is picked.
 */
export const CentralLogin: Component = () => {
  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [companies, setCompanies] = createSignal<Company[]>([]);
  const [address, setAddress] = createSignal("");
  const [captcha, setCaptcha] = createSignal<string | null>(null);
  const [token, setToken] = createSignal<string | null>(null);
  let resetCaptcha = () => {};
  const domain = window.location.hostname.slice(CENTRAL_HOST.length);

  const signIn = useMutation(() => ({
    mutationFn: () =>
      post<{ companies: Company[] }>("/auth/central", {
        email: email(),
        password: password(),
        captcha: token(),
      }),
    onSuccess: ({ companies }) => {
      if (companies.length === 1) window.location.assign(companies[0]!.url);
      else setCompanies(companies);
    },
    onError: async (error) => {
      if (needsCaptcha(error) && !captcha()) setCaptcha(await captchaSiteKey());
      resetCaptcha();
    },
  }));

  const goToAddress = (e: Event) => {
    e.preventDefault();
    const slug = address().trim().toLowerCase();
    if (/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(slug)) {
      window.location.assign(`https://${slug}.${domain}/login`);
    }
  };

  return (
    <main class="login">
      <Show
        when={companies().length > 1}
        fallback={
          <form
            class="login__card"
            onSubmit={(e) => {
              e.preventDefault();
              if (!signIn.isPending) signIn.mutate();
            }}
          >
            <div class="login__brand">
              <img
                class="login__mark"
                src="/logo.svg"
                alt={t("web:app.name")}
                width="56"
                height="56"
              />
            </div>
            <h1 class="login__title">{t("web:login.title")}</h1>
            <p class="login__tagline">{t("web:signInPage.intro")}</p>
            <Show when={signIn.isError}>
              <p class="login__alert" role="alert">
                {errorMessage(signIn.error)}
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
              <span>{t("web:common.password")}</span>
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
              disabled={signIn.isPending || signIn.isSuccess || (!!captcha() && !token())}
            >
              {signIn.isPending || signIn.isSuccess
                ? t("web:login.submitting")
                : t("web:login.submit")}
            </button>

            <div class="login__divider">
              <span>{t("web:signInPage.orByAddress")}</span>
            </div>
            <p class="login__tagline">{t("web:signInPage.addressHelp")}</p>
            <div class="login__field login__field--inline">
              <input
                name="address"
                aria-label={t("web:signInPage.address")}
                placeholder={t("web:signInPage.addressExample")}
                value={address()}
                onInput={(e) => setAddress(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") goToAddress(e);
                }}
              />
              <span class="login__suffix">.{domain}</span>
              <button class="login__provider" type="button" onClick={goToAddress}>
                {t("web:signInPage.go")}
              </button>
            </div>
          </form>
        }
      >
        <div class="login__card">
          <div class="login__brand">
            <img
              class="login__mark"
              src="/logo.svg"
              alt={t("web:app.name")}
              width="56"
              height="56"
            />
          </div>
          <h1 class="login__title">{t("web:signInPage.pick")}</h1>
          <p class="login__tagline">{t("web:signInPage.pickHelp")}</p>
          <div class="login__providers">
            <For each={companies()}>
              {(company) => (
                <a class="login__provider" href={company.url}>
                  {company.name}
                  <span class="login__suffix">
                    {" "}
                    {company.slug}.{domain}
                  </span>
                </a>
              )}
            </For>
          </div>
        </div>
      </Show>
      <LangSwitch class="lang-switch lang-switch--login" />
    </main>
  );
};

/**
 * `/handoff#<pass>` on the company's subdomain: redeems the common entrance's
 * pass for the session and opens the orchestrator.
 */
export const Handoff: Component = () => {
  const [failed, setFailed] = createSignal(false);
  onMount(() => {
    const pass = window.location.hash.slice(1);
    // The pass must not stay in the history.
    history.replaceState(null, "", "/handoff");
    post("/auth/handoff", { token: pass }).then(
      () => window.location.replace("/"),
      () => setFailed(true),
    );
  });
  return (
    <main class="login">
      <Show
        when={failed()}
        fallback={
          <div class="login__opening">
            <Spiral />
            <p role="status">{t("web:login.submitting")}</p>
          </div>
        }
      >
        <div class="login__card">
          <h1 class="login__title">{t("web:signInPage.linkExpired")}</h1>
          <a class="login__submit" href="/login">
            {t("web:login.submit")}
          </a>
        </div>
      </Show>
    </main>
  );
};
