import { type Component } from "solid-js";
// One icon by its own path, as in Modal.
import AlertTriangle from "lucide-solid/icons/triangle-alert";
import { Modal } from "./Modal";
import { t } from "./i18n";

/**
 * Destructive-action confirmation dialog on top of {@link Modal}: warning
 * message plus cancel/confirm buttons with a pending state. Replaces
 * `window.confirm` so the warning is styled, keyboard-dismissable, and
 * consistent across features.
 */
export const ConfirmDialog: Component<{
  open: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  pendingLabel?: string;
  pending?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}> = (props) => (
  <Modal open={props.open} title={props.title} onClose={() => props.onCancel()}>
    <p class="fk-confirm__message">
      <AlertTriangle size={20} class="fk-confirm__icon" />
      <span>{props.message}</span>
    </p>
    <div class="fk-confirm__actions">
      <button
        type="button"
        class="fk-btn"
        disabled={props.pending}
        onClick={() => props.onCancel()}
      >
        {props.cancelLabel ?? t("kit:cancel")}
      </button>
      <button
        type="button"
        class="fk-btn fk-btn--danger"
        disabled={props.pending}
        onClick={() => props.onConfirm()}
      >
        {props.pending
          ? (props.pendingLabel ?? t("kit:deleting"))
          : (props.confirmLabel ?? t("kit:delete"))}
      </button>
    </div>
  </Modal>
);
