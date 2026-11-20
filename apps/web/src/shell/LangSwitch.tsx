import { For, type Component } from "solid-js";
import { LANGS, lang, setLang, t, type Lang } from "@knime-orchestador/i18n";

/**
 * Choose the UI language. With `onSave` (inside the session) it also saves it
 * in the account for the other devices. The texts change at once, without
 * reloading the page (the app is rebuilt in the other language).
 */
export const LangSwitch: Component<{
  onSave?: (lang: Lang) => Promise<unknown>;
  class?: string;
}> = (props) => (
  <div class={props.class ?? "lang-switch"} role="group" aria-label={t("web:shell.language")}>
    <For each={LANGS}>
      {(l) => (
        <button
          type="button"
          classList={{ "lang-switch__option": true, "lang-switch__option--on": lang() === l }}
          aria-pressed={lang() === l}
          lang={l}
          onClick={async () => {
            if (lang() === l) return;
            setLang(l);
            await props.onSave?.(l);
          }}
        >
          {t(`web:lang.${l}`)}
        </button>
      )}
    </For>
  </div>
);
