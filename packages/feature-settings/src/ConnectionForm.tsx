import { For, Show, type Component } from "solid-js";
import { Field, SecretInput } from "./common";
import { engineById, ENGINES, GROUPS, type ConnectionField } from "./connections";
import { t } from "./i18n";

/**
 * The fields of a database connection: the engine (grouped: SQL, cloud,
 * NoSQL, file, other) and that engine's form. The secret fields already saved
 * (`saved`) show empty with «guardado»: if they are not written, they are
 * kept.
 */
export const ConnectionForm: Component<{
  engine: string;
  onEngine: (engine: string) => void;
  fields: Record<string, string>;
  onField: (key: string, value: string) => void;
  /** The secret fields it already has saved (when editing). */
  saved?: string[];
  /** When editing, the engine does not change (it would be another connection). */
  lockEngine?: boolean;
  errors?: Record<string, string | undefined>;
}> = (props) => {
  const engine = () => engineById(props.engine);
  const label = (f: ConnectionField) => t(`settings:secrets.conn.field.${f.key}`);
  const input = (f: ConnectionField) => {
    const value = () => props.fields[f.key] ?? "";
    const saved = () => props.saved?.includes(f.key) ?? false;
    if (f.kind === "secret")
      return (
        <SecretInput
          value={value()}
          saved={saved()}
          required={f.required && !saved()}
          placeholder={saved() ? undefined : f.placeholder}
          ariaLabel={label(f)}
          onInput={(v) => props.onField(f.key, v)}
        />
      );
    if (f.kind === "textarea")
      return (
        <textarea
          class="st-mono"
          rows={4}
          spellcheck={false}
          required={f.required && !saved()}
          placeholder={saved() ? t("settings:common.secretSaved") : f.placeholder}
          aria-label={label(f)}
          value={value()}
          onInput={(e) => props.onField(f.key, e.currentTarget.value)}
        />
      );
    return (
      <input
        class={f.kind === "port" ? "st-mono" : undefined}
        inputmode={f.kind === "port" ? "numeric" : undefined}
        autocomplete="off"
        spellcheck={false}
        required={f.required}
        placeholder={f.kind === "port" ? String(engine()?.port ?? "") : (f.placeholder ?? "")}
        aria-label={label(f)}
        value={value()}
        onInput={(e) => props.onField(f.key, e.currentTarget.value)}
      />
    );
  };
  return (
    <div class="st-conn">
      <Field label={t("settings:secrets.conn.engine")}>
        <select
          required
          disabled={props.lockEngine}
          value={props.engine}
          onChange={(e) => props.onEngine(e.currentTarget.value)}
        >
          <option value="">{t("settings:secrets.conn.pickEngine")}</option>
          <For each={GROUPS}>
            {(g) => (
              <optgroup label={t(`settings:secrets.conn.group.${g}`)}>
                <For each={ENGINES.filter((e) => e.group === g)}>
                  {(e) => <option value={e.id}>{e.label}</option>}
                </For>
              </optgroup>
            )}
          </For>
        </select>
      </Field>
      <Show when={engine()}>
        {(e) => (
          <div class="st-conn__fields">
            <For each={e().fields}>
              {(f) => (
                <div
                  classList={{
                    "st-conn__wide": f.kind === "textarea" || f.key === "connection_string",
                  }}
                >
                  <Field
                    label={label(f) + (f.required ? "" : ` ${t("settings:secrets.conn.optional")}`)}
                    error={props.errors?.[f.key]}
                  >
                    {input(f)}
                  </Field>
                </div>
              )}
            </For>
          </div>
        )}
      </Show>
    </div>
  );
};
