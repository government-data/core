import { createSignal, For, Match, Show, Switch, type Component } from "solid-js";
import { Modal } from "@knime-orchestador/feature-kit/ui";
import { t } from "./i18n";
import {
  barState,
  forget,
  keep,
  kept,
  PURPOSES,
  reasonOf,
  referenceProblem,
  type Reason,
  type ViewPurpose,
} from "./reveal";

/**
 * Why values are to be shown (decision 0069, points 6 and 7): a purpose of
 * the fixed list and a ticket or case number, checked before it is sent and
 * required with _Other_.
 */
export const RevealDialog: Component<{
  initial?: Reason;
  onShow: (reason: Reason) => void;
  onClose: () => void;
}> = (props) => {
  const [purpose, setPurpose] = createSignal<ViewPurpose>(
    props.initial?.purpose ?? "quality_review",
  );
  const [reference, setReference] = createSignal(props.initial?.reference ?? "");
  const [tried, setTried] = createSignal(false);
  const problem = () => referenceProblem(purpose(), reference());
  return (
    <Modal open title={t("quality:reveal.title")} onClose={props.onClose}>
      <form
        class="qd-form"
        onSubmit={(e) => {
          e.preventDefault();
          setTried(true);
          if (!problem()) props.onShow(reasonOf(purpose(), reference()));
        }}
      >
        <p class="qd-hint">{t("quality:reveal.intro")}</p>
        <label class="qd-field">
          <span>{t("quality:reveal.purpose")}</span>
          <select
            ref={(el) => queueMicrotask(() => el.focus())}
            value={purpose()}
            onChange={(e) => setPurpose(e.currentTarget.value as ViewPurpose)}
          >
            <For each={PURPOSES}>
              {(p) => <option value={p}>{t(`quality:reveal.purposes.${p}`)}</option>}
            </For>
          </select>
        </label>
        <label class="qd-field">
          <span>
            {purpose() === "other"
              ? t("quality:reveal.referenceRequired")
              : t("quality:reveal.reference")}
          </span>
          <input
            class="qd-mono"
            autocomplete="off"
            spellcheck={false}
            maxLength={32}
            value={reference()}
            onInput={(e) => setReference(e.currentTarget.value)}
          />
          <small>{t("quality:reveal.referenceHint")}</small>
        </label>
        <Show when={tried() && problem()}>
          {(p) => (
            <p class="qd-error" role="alert">
              {t(`quality:reveal.invalid.${p()}`)}
            </p>
          )}
        </Show>
        <div class="qd-actions">
          <button type="button" class="fk-btn" onClick={props.onClose}>
            {t("quality:common.cancel")}
          </button>
          <button type="submit" class="fk-btn fk-btn--primary">
            {t("quality:reveal.show")}
          </button>
        </div>
      </form>
    </Modal>
  );
};

/**
 * Above values that may be shown (decision 0069, point 5, and the ruling of
 * 2026-11-13), as `barState` says: "Showing values for: …" with _Change_ and
 * _Mask again_ only while the values are whole; while they are masked,
 * "Values masked." with _Show values_, or, with a reason kept (as after a
 * refused reveal), _Change_ to try again or give another and _Forget
 * reason_; and why a reveal was refused.
 */
export const RevealBar: Component<{
  scope: string;
  revealable: boolean;
  masked: boolean;
  busy?: boolean;
  refused?: string;
  /** Shows the values with `reason`, kept already. */
  onReveal: (reason: Reason) => void;
  /** Shows the values masked again; the reason is forgotten already. */
  onMask: () => void;
}> = (props) => {
  const [asking, setAsking] = createSignal(false);
  const reason = () => kept(props.scope);
  const show = (r: Reason) => {
    setAsking(false);
    keep(props.scope, r);
    if (props.masked && props.revealable) props.onReveal(r);
  };
  const state = () => barState(!!reason(), props.revealable, props.masked);
  return (
    <div class="qd-reveal">
      <Switch>
        <Match when={state() === "showing" && reason()}>
          {(r) => (
            <>
              <span class="qd-hint">
                {t("quality:reveal.showing", {
                  purpose: t(`quality:reveal.purposes.${r().purpose}`),
                })}
                {r().reference ? `, ${r().reference}` : ""}
              </span>{" "}
              <button type="button" class="fk-btn fk-btn--sm" onClick={() => setAsking(true)}>
                {t("quality:reveal.change")}
              </button>{" "}
              <button
                type="button"
                class="fk-btn fk-btn--sm"
                onClick={() => {
                  forget(props.scope);
                  props.onMask();
                }}
              >
                {t("quality:reveal.mask")}
              </button>
            </>
          )}
        </Match>
        <Match when={state() === "ask" || state() === "retry"}>
          <span class="qd-hint">{t("quality:reveal.masked")}</span>{" "}
          <button
            type="button"
            class="fk-btn fk-btn--sm"
            disabled={props.busy}
            onClick={() => setAsking(true)}
          >
            {state() === "retry" ? t("quality:reveal.change") : t("quality:reveal.show")}
          </button>
          <Show when={state() === "retry"}>
            {" "}
            <button
              type="button"
              class="fk-btn fk-btn--sm"
              disabled={props.busy}
              onClick={() => forget(props.scope)}
            >
              {t("quality:reveal.forget")}
            </button>
          </Show>
        </Match>
      </Switch>
      <Show when={props.refused}>
        {(why) => (
          <p class="qd-error" role="alert">
            {t(`quality:reveal.refused.${why()}`, {
              defaultValue: t("quality:reveal.refused.other"),
            })}
          </p>
        )}
      </Show>
      <Show when={asking()}>
        <RevealDialog initial={reason()} onShow={show} onClose={() => setAsking(false)} />
      </Show>
    </div>
  );
};
