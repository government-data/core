import { createEffect, on, type Component } from "solid-js";
import { toast } from "@knime-orchestador/feature-kit/ui";
import { t } from "./i18n";

/** An action's outcome as a toast: a success fades; an error, or a warning, stays until closed. */
export const notify = (text: string, bad = false) =>
  bad ? toast("error", text, undefined, null) : toast("success", text);

/** A refusal that is no error of the person's, as a warning that stays until closed. */
export const warn = (text: string) => toast("warning", text, undefined, null);

/** When something failed to load: an error toast, once each time it fails. */
export const LoadFailed: Component<{ when: unknown }> = (props) => {
  createEffect(
    on(
      () => !!props.when,
      (failed) => {
        if (failed) notify(t("quality:common.loadError"), true);
      },
    ),
  );
  return null;
};
