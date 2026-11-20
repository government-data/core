import { createSignal, onCleanup, Show, type Component } from "solid-js";
import { formatDateTime, formatNumber } from "@knime-orchestador/i18n";
import { Spiral } from "@knime-orchestador/feature-kit/ui";
import type { ListingView } from "./api";
import { t } from "./i18n";

/** What a `tables` job does, in its order: shown in turn while it runs. */
const STEPS = ["connect", "schemas", "columns", "tables"] as const;
const STEP_MS = 1600;

/** Where a source's listing stands, in the person's words (decision 0042). */
export const ListingStatus: Component<{ listing: ListingView }> = (props) => (
  <>
    <Show when={props.listing.state === "listing"}>
      <ListingSteps />
    </Show>
    <Show when={props.listing.state === "retrying"}>
      <p class="qd-hint" role="status">
        <Spiral class="qd-spiral-sm" />
        {t("quality:sources.tables.retrying", {
          why: t(`quality:sources.tables.reason.${props.listing.reason ?? "other"}`),
          n: props.listing.attempt ?? 2,
          max: props.listing.max_attempts ?? 3,
        })}
      </p>
    </Show>
    <Show when={props.listing.state === "failed"}>
      <p class="qd-error">
        <Show when={props.listing.reason}>
          {(why) => <>{t(`quality:sources.tables.reason.${why()}`)} </>}
        </Show>
        {t("quality:sources.tables.failed")}
      </p>
    </Show>
    <Show when={props.listing.state === "none"}>
      <p class="qd-hint">{t("quality:sources.tables.none")}</p>
    </Show>
    <Show when={props.listing.listed_at}>
      {(at) => (
        <p class="qd-hint">
          {t("quality:sources.tables.listedAt", {
            count: props.listing.tables.length,
            n: formatNumber(props.listing.tables.length),
            at: formatDateTime(at()),
          })}
          <Show when={props.listing.truncated}> {t("quality:sources.tables.truncated")}</Show>
        </p>
      )}
    </Show>
  </>
);

/**
 * A job on its way, step by step. The worker does not say which step it is
 * on: the steps turn in its order, the last one held until it is done.
 */
const ListingSteps: Component = () => {
  const [step, setStep] = createSignal(0);
  const timer = setInterval(() => setStep((s) => Math.min(s + 1, STEPS.length - 1)), STEP_MS);
  onCleanup(() => clearInterval(timer));
  return (
    <p class="qd-hint" role="status">
      <Spiral class="qd-spiral-sm" />
      <Show when={STEPS[step()]} keyed>
        {(s) => <span class="qd-step">{t(`quality:sources.tables.step.${s}`)}</span>}
      </Show>
    </p>
  );
};
