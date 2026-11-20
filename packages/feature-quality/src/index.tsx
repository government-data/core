import { createResource, createRoot, lazy, type Component, type Resource } from "solid-js";
import { defineFeature, onSignOut } from "@knime-orchestador/feature-kit";
import { may, me, type Me } from "./api";
import { t } from "./i18n";
import { forgetAll } from "./reveal";

// A view's reasons kept in the tab end with the session (decision 0069, point 5).
onSignOut(forgetAll);

const view = <M,>(load: () => Promise<M>, pick: (m: M) => Component) =>
  lazy(() => load().then((m) => ({ default: pick(m) })));

// What the person may do, read the first time the sidebar asks (not on the
// sign-in page) and owned by a root of its own; the API checks again.
let actions: Resource<Me> | undefined;
const who = () => (actions ??= createRoot(() => createResource(me)[0]))();

/**
 * Quality: the sources (0036), the cuts and schedules (0035), the rules
 * (0019) and the drafts (0021, 0038), reached from the explorer, its panel
 * in the sidebar (0043), with no entries of its own; each page checks again.
 */
export const qualityFeature = defineFeature({
  id: "quality",
  get title() {
    return t("quality:nav.sources");
  },
  panel: {
    get label() {
      return t("quality:explorer.title");
    },
    component: view(
      () => import("./Explorer"),
      (m) => m.Explorer,
    ),
    visible: () => may(who(), "quality:ViewRules") || may(who(), "quality:ViewSources"),
  },
  routes: [
    {
      path: "/quality/sources",
      component: view(
        () => import("./SourcesPage"),
        (m) => m.SourcesPage,
      ),
    },
    {
      path: "/quality/rules",
      component: view(
        () => import("./RulesPage"),
        (m) => m.RulesPage,
      ),
    },
    {
      path: "/quality/rules/:dataset/:file",
      component: view(
        () => import("./RuleEditorPage"),
        (m) => m.RuleEditorPage,
      ),
    },
    {
      path: "/quality/table",
      component: view(
        () => import("./TablePage"),
        (m) => m.TablePage,
      ),
    },
    {
      path: "/quality/dashboard/:dataset",
      component: view(
        () => import("./DashboardPage"),
        (m) => m.DashboardPage,
      ),
    },
    {
      path: "/quality/records",
      component: view(
        () => import("./RecordsPage"),
        (m) => m.RecordsPage,
      ),
    },
  ],
});

export default qualityFeature;
