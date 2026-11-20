import { Navigate } from "@solidjs/router";
import { lazy, type Component } from "solid-js";
import { defineFeature } from "@knime-orchestador/feature-kit";

const view = <M,>(load: () => Promise<M>, pick: (m: M) => Component) =>
  lazy(() => load().then((m) => ({ default: pick(m) })));

/**
 * The platform: the updates, with the toasts and the maintenance notice every
 * section shows.
 */
export const platformFeature = defineFeature({
  id: "platform",
  title: "Platform",
  // The toasts, in sight from any section.
  overlay: view(
    () => import("./Overlay"),
    (m) => m.PlatformOverlay,
  ),
  routes: [
    // Until the quality screens exist (ADR 0016), the start is Settings.
    { path: "/", component: () => <Navigate href="/settings" /> },
    // An application of this installation (RustFS…), inside Quality Data.
    {
      path: "/apps/:id",
      component: view(
        () => import("./AppFrame"),
        (m) => m.AppFrame,
      ),
    },
    {
      path: "/settings/updates",
      component: view(
        () => import("./Updates"),
        (m) => m.Updates,
      ),
    },
  ],
});

export default platformFeature;
