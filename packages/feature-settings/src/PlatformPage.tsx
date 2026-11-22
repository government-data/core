import "./settings.css";
import { createResource, createSignal, For, Show, type Component } from "solid-js";
import { settingsApi, type PlatformSettings, type Section, type Source } from "./api";
import { Field, NoticeBox, SectionHeader, SourceNote, useAction } from "./common";
import { t } from "./i18n";

/** The IANA names the browser knows, to suggest while typing. */
const ZONES: string[] = (() => {
  const intl = Intl as { supportedValuesOf?: (key: string) => string[] };
  try {
    return intl.supportedValuesOf?.("timeZone") ?? [];
  } catch {
    return [];
  }
})();

/** Settings → Platform: the installation's time zone (decision 0079). */
export const PlatformPage: Component = () => {
  const [timezone, setTimezone] = createSignal("UTC");
  const [source, setSource] = createSignal<Source>();
  const save = useAction();

  const load = (s: Section<PlatformSettings>) => {
    setTimezone(s.value.timezone);
    setSource(s.source);
  };

  const [settings] = createResource(async () => {
    const all = await settingsApi.all();
    load(all.platform);
    return all;
  });

  return (
    <section class="feature">
      <SectionHeader title={t("settings:platform.title")} subtitle={t("settings:platform.intro")} />
      <Show when={settings.error}>
        <p class="st-notice st-notice--error">{t("settings:platform.readError")}</p>
      </Show>
      <Show when={source()}>
        {(src) => (
          <SourceNote
            source={src()}
            onReset={() =>
              save.run(async () => {
                const all = await settingsApi.reset("platform");
                load(all.platform);
                return t("settings:platform.reset");
              })
            }
          />
        )}
      </Show>

      <form
        class="card st-form"
        onSubmit={(e) => {
          e.preventDefault();
          void save.run(async () => {
            load(await settingsApi.savePlatform({ timezone: timezone() }));
            return t("settings:platform.saved");
          });
        }}
      >
        <Field
          label={t("settings:platform.timezone")}
          error={save.fields()["timezone"]}
          tip={t("settings:platform.timezoneHelp")}
          hint={t("settings:platform.timezoneHint")}
        >
          <input
            value={timezone()}
            list="st-timezones"
            placeholder="America/Bogota"
            onInput={(e) => setTimezone(e.currentTarget.value)}
          />
        </Field>
        <datalist id="st-timezones">
          <For each={ZONES}>{(zone) => <option value={zone} />}</For>
        </datalist>
        <div class="st-actions">
          <button type="submit" class="fk-btn st-primary" disabled={save.busy()}>
            {save.busy() ? t("settings:platform.saving") : t("settings:platform.save")}
          </button>
        </div>
        <NoticeBox notice={save.notice()} />
      </form>
    </section>
  );
};
