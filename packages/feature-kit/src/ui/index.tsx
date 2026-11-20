/**
 * Shared UI primitives for feature packages. Shipped as Solid source (like
 * `@knime-orchestador/query`'s `./provider` entry) so the host app's vite-plugin-solid
 * compiles the JSX and bundles the styles.
 */

// The base styles (.fk-btn and those of these components) with any import
// from here, not only when a dialog is used: the app's shell always imports
// it, so a button looks right in the first view opened.
import "./ui.css";

export { Modal } from "./Modal";
export { ConfirmDialog } from "./ConfirmDialog";
export { Leaving, Spiral } from "./Spiral";
export { Status } from "./Status";
export { dismiss, toast, Toaster, type ToastKind } from "./Toast";
export { statusIcon, type Tone } from "./status";
