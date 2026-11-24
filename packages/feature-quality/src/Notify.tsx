import { createEffect, on, Show, type Component } from "solid-js";
import { toast } from "@knime-orchestador/feature-kit/ui";
import { noCatalog } from "./apiCore";
import { t } from "./i18n";

/** An action's outcome as a toast: a success fades; an error, or a warning, stays until closed. */
export const notify = (text: string, bad = false) =>
  bad ? toast("error", text, undefined, null) : toast("success", text);

/** A refusal that is no error of the person's, as a warning that stays until closed. */
export const warn = (text: string) => toast("warning", text, undefined, null);

/** The installation has no catalog: said on the page, not as a failure. */
export const NoCatalog: Component = () => (
  <p class="qd-hint qd-no-catalog" role="status">
    {t("quality:common.noCatalog")}
  </p>
);

/**
 * When something failed to load: an error toast, once each time it fails;
 * when it is because there is no catalog, {@link NoCatalog} instead.
 */
export const LoadFailed: Component<{ when: unknown }> = (props) => {
  createEffect(
    on(
      () => !!props.when && !noCatalog(props.when),
      (failed) => {
        if (failed) notify(t("quality:common.loadError"), true);
      },
    ),
  );
  return (
    <Show when={noCatalog(props.when)}>
      <NoCatalog />
    </Show>
  );
};
