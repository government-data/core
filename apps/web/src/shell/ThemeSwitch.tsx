import { For, type Component } from "solid-js";
import { t } from "@knime-orchestador/i18n";
import { setTheme, theme, THEMES } from "./theme";

/** Choose the theme, drawn as the language's switch (LangSwitch). */
export const ThemeSwitch: Component = () => (
  <div class="lang-switch" role="group" aria-label={t("web:shell.theme")}>
    <For each={THEMES}>
      {(choice) => (
        <button
          type="button"
          classList={{ "lang-switch__option": true, "lang-switch__option--on": theme() === choice }}
          aria-pressed={theme() === choice}
          onClick={() => setTheme(choice)}
        >
          {t(`web:theme.${choice}`)}
        </button>
      )}
    </For>
  </div>
);
