import { For, type Component } from "solid-js";
import { t } from "./i18n";
import { span } from "./trigger";
import { RANDOM_DELAYS, REPEAT_EVERY, REPEAT_FOR } from "./triggerForm";
import type { WhenProps } from "./WhenFields";

const w = (key: string) => t(`quality:schedules.whenForm.${key}`);

/** A random delay, repetition and expiry, each with its box, as in Windows. */
export const Advanced: Component<WhenProps> = (props) => (
  <fieldset class="qd-sched__advanced">
    <legend>{w("advanced")}</legend>
    <div class="qd-sched__opt">
      <label class="qd-check">
        <input
          type="checkbox"
          checked={props.when.delayOn}
          onChange={(e) => props.set("delayOn", e.currentTarget.checked)}
        />
        {w("randomDelay")}
      </label>
      <select
        disabled={!props.when.delayOn}
        value={props.when.delay}
        onChange={(e) => props.set("delay", Number(e.currentTarget.value))}
      >
        <For each={RANDOM_DELAYS}>{(s) => <option value={s}>{span(s)}</option>}</For>
      </select>
    </div>
    <div class="qd-sched__opt">
      <label class="qd-check">
        <input
          type="checkbox"
          checked={props.when.repeatOn}
          onChange={(e) => props.set("repeatOn", e.currentTarget.checked)}
        />
        {w("repeatEvery")}
      </label>
      <select
        disabled={!props.when.repeatOn}
        value={props.when.repeatEvery}
        onChange={(e) => props.set("repeatEvery", Number(e.currentTarget.value))}
      >
        <For each={REPEAT_EVERY}>{(m) => <option value={m}>{span(m * 60)}</option>}</For>
      </select>
      <span class="qd-hint">{w("during")}</span>
      <select
        disabled={!props.when.repeatOn}
        value={props.when.repeatFor ?? ""}
        onChange={(e) =>
          props.set(
            "repeatFor",
            e.currentTarget.value === "" ? null : Number(e.currentTarget.value),
          )
        }
      >
        <For each={REPEAT_FOR}>
          {(m) => <option value={m ?? ""}>{m === null ? w("forever") : span(m * 60)}</option>}
        </For>
      </select>
    </div>
    <div class="qd-sched__opt">
      <label class="qd-check">
        <input
          type="checkbox"
          checked={props.when.expiresOn}
          onChange={(e) => props.set("expiresOn", e.currentTarget.checked)}
        />
        {w("expires")}
      </label>
      <input
        type="date"
        disabled={!props.when.expiresOn}
        value={props.when.expiresDate}
        onInput={(e) => props.set("expiresDate", e.currentTarget.value)}
      />
      <input
        type="time"
        disabled={!props.when.expiresOn}
        value={props.when.expiresTime}
        onInput={(e) => props.set("expiresTime", e.currentTarget.value)}
      />
    </div>
  </fieldset>
);
