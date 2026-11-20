import { onCleanup, onMount, type Component } from "solid-js";
import { ApiError } from "@knime-orchestador/query";
import { lang } from "@knime-orchestador/i18n";

/** What the Turnstile script puts on `window`. */
interface TurnstileApi {
  render(
    el: HTMLElement,
    options: {
      sitekey: string;
      language?: string;
      callback: (token: string) => void;
      "expired-callback": () => void;
      "error-callback": () => void;
    },
  ): string;
  reset(id: string): void;
  remove(id: string): void;
}

declare global {
  interface Window {
    turnstile?: TurnstileApi;
  }
}

let loading: Promise<TurnstileApi> | null = null;

/** The Turnstile script, only once (the CSP allows it only from Cloudflare). */
function load(): Promise<TurnstileApi> {
  loading ??= new Promise((resolve, reject) => {
    if (window.turnstile) return resolve(window.turnstile);
    const s = document.createElement("script");
    s.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    s.async = true;
    s.onload = () =>
      window.turnstile ? resolve(window.turnstile) : reject(new Error("turnstile"));
    s.onerror = () => {
      loading = null;
      reject(new Error("turnstile"));
    };
    document.head.append(s);
  });
  return loading;
}

/** Whether the API asked for the captcha (`captcha_required`). */
export function needsCaptcha(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 403 &&
    (error.body as { code?: string } | undefined)?.code === "captcha_required"
  );
}

/**
 * Cloudflare's captcha (Turnstile): almost always invisible; it hands out a
 * single-use token the server verifies. `bindReset` receives the function to
 * call after every submit to get a new one.
 */
export const Turnstile: Component<{
  siteKey: string;
  onToken: (token: string | null) => void;
  bindReset?: (fn: () => void) => void;
}> = (props) => {
  let box!: HTMLDivElement;
  let id: string | undefined;
  onMount(() => {
    void load()
      .then((t) => {
        id = t.render(box, {
          sitekey: props.siteKey,
          language: lang(),
          callback: (token) => props.onToken(token),
          "expired-callback": () => props.onToken(null),
          "error-callback": () => props.onToken(null),
        });
        props.bindReset?.(() => {
          props.onToken(null);
          if (id) t.reset(id);
        });
      })
      // Without Cloudflare (blocked or offline), the server will say if it was needed.
      .catch(() => props.onToken(null));
  });
  onCleanup(() => {
    if (id) window.turnstile?.remove(id);
  });
  return <div class="login__captcha" ref={box} />;
};
