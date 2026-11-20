import { createSignal, createUniqueId, Show, type Component, type JSX } from "solid-js";
import { A } from "@solidjs/router";
import { SettingsApiError, type Source } from "./api";
import { t } from "./i18n";
import { formatDateTime } from "@knime-orchestador/i18n";

/** A section's header, with a way back to the Settings home page. */
/**
 * A section's content: a page of its own, with its title, or (`embedded`) a
 * tab of a page that already has one, with only its intro.
 */
export const Panel: Component<{
  embedded?: boolean;
  title: string;
  subtitle: string;
  children: JSX.Element;
}> = (props) => (
  <Show
    when={props.embedded}
    fallback={
      <section class="feature">
        <SectionHeader title={props.title} subtitle={props.subtitle} />
        {props.children}
      </section>
    }
  >
    <div class="st-panel">
      <p class="st-panel__intro">{props.subtitle}</p>
      {props.children}
    </div>
  </Show>
);

export const SectionHeader: Component<{ title: string; subtitle: string }> = (props) => (
  <header class="feature__header">
    <A href="/settings" class="st-back">
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M19 12H5M11 18l-6-6 6-6" />
      </svg>
      {t("settings:common.back")}
    </A>
    <h1 class="feature__title">{props.title}</h1>
    <p class="feature__subtitle">{props.subtitle}</p>
  </header>
);

/** Where the settings in use come from. */
export const SourceNote: Component<{ source: Source; onReset?: () => void }> = (props) => (
  <p class="st-source">
    <Show when={props.source.kind === "database"}>
      {t("settings:common.savedHere")}
      <Show when={props.source.updated_at}>
        {(at) => <>{t("settings:common.on", { date: formatDateTime(at()) })}</>}
      </Show>
      <Show when={props.source.updated_by}>
        {(by) => <>{t("settings:common.by", { who: by() })}</>}
      </Show>
      .
      <Show when={props.onReset}>
        {" "}
        <button type="button" class="st-link" onClick={() => props.onReset?.()}>
          {t("settings:common.backToEnv")}
        </button>
      </Show>
    </Show>
    <Show when={props.source.kind === "environment"}>{t("settings:common.fromEnv")}</Show>
    <Show when={props.source.kind === "unset"}>{t("settings:common.unset")}</Show>
  </p>
);

/**
 * A "?" that explains a field: it opens on mouse hover, keyboard focus or a
 * tap, and closes with Escape or on leaving. It goes inside a `<label>`
 * without stealing the control's click.
 */
export const Tip: Component<{ children: JSX.Element }> = (props) => {
  const id = createUniqueId();
  const [open, setOpen] = createSignal(false);
  let bubble: HTMLSpanElement | undefined;
  // Shift it so it does not leave the screen (to the right on a phone).
  const place = () =>
    requestAnimationFrame(() => {
      if (!bubble) return;
      bubble.style.translate = "";
      const r = bubble.getBoundingClientRect();
      if (r.width === 0) return;
      const margin = 8;
      // The visible width: innerWidth grows on a phone if something overflows.
      const width = document.documentElement.clientWidth;
      let dx = 0;
      if (r.right > width - margin) dx = width - margin - r.right;
      if (r.left + dx < margin) dx = margin - r.left;
      bubble.style.translate = `${dx}px 0`;
    });
  return (
    <span class="st-tip" classList={{ "st-tip--open": open() }} onMouseEnter={place}>
      <button
        type="button"
        class="st-tip__btn"
        aria-label={t("settings:common.help")}
        aria-describedby={id}
        aria-expanded={open()}
        onClick={(e) => {
          // Inside a <label>, the click must not activate the control.
          e.preventDefault();
          e.stopPropagation();
          setOpen(!open());
          place();
        }}
        onFocus={place}
        onBlur={() => setOpen(false)}
        onKeyDown={(e) => {
          if (e.key === "Escape") setOpen(false);
        }}
      >
        ?
      </button>
      <span ref={bubble} role="tooltip" id={id} class="st-tip__bubble">
        {props.children}
      </span>
    </span>
  );
};

/**
 * A label with its control and, if any, the validation error. `tip` puts a
 * "?" beside the label; it goes after the control in the DOM so the `<label>`
 * keeps pointing at the control.
 */
export const Field: Component<{
  label: string;
  error?: string;
  hint?: JSX.Element;
  tip?: JSX.Element;
  children: JSX.Element;
}> = (props) => (
  <label
    class="st-field"
    classList={{ "st-field--invalid": !!props.error, "st-field--tip": !!props.tip }}
  >
    <span class="st-field__label">{props.label}</span>
    {props.children}
    <Show when={props.tip}>
      <Tip>{props.tip}</Tip>
    </Show>
    <Show when={props.error}>
      <span class="st-field__error">{props.error}</span>
    </Show>
    <Show when={props.hint && !props.error}>
      <span class="st-field__hint">{props.hint}</span>
    </Show>
  </label>
);

/**
 * A secret: empty keeps the saved one. Hidden by default; the eye shows it
 * while typing (and hides it again).
 */
export const SecretInput: Component<{
  value: string;
  saved?: boolean;
  onInput: (v: string) => void;
  autocomplete?: string;
  placeholder?: string;
  required?: boolean;
  ariaLabel?: string;
  ref?: (el: HTMLInputElement) => void;
}> = (props) => {
  const [shown, setShown] = createSignal(false);
  return (
    <span class="st-secret">
      <input
        ref={props.ref}
        type={shown() ? "text" : "password"}
        autocomplete={props.autocomplete ?? "new-password"}
        spellcheck={false}
        required={props.required}
        aria-label={props.ariaLabel}
        value={props.value}
        placeholder={props.placeholder ?? (props.saved ? t("settings:common.secretSaved") : "")}
        onInput={(e) => props.onInput(e.currentTarget.value)}
      />
      <button
        type="button"
        class="st-secret__toggle"
        aria-pressed={shown()}
        aria-label={shown() ? t("settings:common.hideSecret") : t("settings:common.showSecret")}
        title={shown() ? t("settings:common.hideSecret") : t("settings:common.showSecret")}
        // Without stealing the field's focus: typing goes on.
        onMouseDown={(e) => e.preventDefault()}
        onClick={(e) => {
          // Inside a <label>: the click must not go to the field.
          e.preventDefault();
          setShown(!shown());
        }}
      >
        <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
          <path
            d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
          />
          <circle cx="12" cy="12" r="3" fill="none" stroke="currentColor" stroke-width="1.8" />
          <Show when={shown()}>
            <path d="M4 4l16 16" stroke="currentColor" stroke-width="1.8" />
          </Show>
        </svg>
      </button>
    </span>
  );
};

export const Toggle: Component<{
  checked: boolean;
  onChange: (v: boolean) => void;
  tip?: JSX.Element;
  children: JSX.Element;
}> = (props) => (
  <label class="st-toggle">
    <input
      type="checkbox"
      checked={props.checked}
      onChange={(e) => props.onChange(e.currentTarget.checked)}
    />
    <span>{props.children}</span>
    <Show when={props.tip}>
      <Tip>{props.tip}</Tip>
    </Show>
  </label>
);

export type Notice = { kind: "ok" | "error"; text: string };

export const NoticeBox: Component<{ notice?: Notice }> = (props) => (
  <Show when={props.notice}>
    {(n) => (
      <p class="st-notice" classList={{ "st-notice--error": n().kind === "error" }} role="status">
        {n().text}
      </p>
    )}
  </Show>
);

/**
 * Run a form action: marks "busy", keeps the errors per field and leaves a
 * notice with the result.
 */
export function useAction() {
  const [busy, setBusy] = createSignal(false);
  const [fields, setFields] = createSignal<Record<string, string>>({});
  const [notice, setNotice] = createSignal<Notice>();

  async function run(action: () => Promise<string | void>) {
    setBusy(true);
    setNotice(undefined);
    try {
      const ok = await action();
      setFields({});
      if (ok) setNotice({ kind: "ok", text: ok });
    } catch (e) {
      if (e instanceof SettingsApiError) {
        setFields(e.fields);
        setNotice({ kind: "error", text: e.message });
      } else {
        setNotice({ kind: "error", text: String(e) });
      }
    } finally {
      setBusy(false);
    }
  }

  return { busy, fields, notice, run };
}

/** Copy a text, with a brief "Copiado". */
export const CopyButton: Component<{ text: string }> = (props) => {
  const [copied, setCopied] = createSignal(false);
  return (
    <button
      type="button"
      class="fk-btn fk-btn--sm"
      onClick={async () => {
        await navigator.clipboard.writeText(props.text);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied() ? t("settings:common.copied") : t("settings:common.copy")}
    </button>
  );
};

/** A list with one line per item, for a textarea. */
export const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
