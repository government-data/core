/**
 * Settings → Lake's replication (decision 0066 step 11, the user's rulings
 * of 2026-11-13): whether the application can configure the main lake to
 * replicate into the replica, and what its last run left of each company's
 * warehouse, which it makes on the main lake first when missing (the
 * user's ruling of that day). No key reaches this page.
 */

import { formatDateTime } from "@knime-orchestador/i18n";
import { t } from "./i18n";
import type { ReplicaLine } from "./lakeReplica";
import type { Replication, WarehouseReplication } from "./machinesApi";

/** Whether it can run, in one line. */
export function readinessLine(r: Replication): ReplicaLine {
  switch (r.readiness) {
    case "ready":
      return {
        tone: "ok",
        text: t("settings:lake.replication.ready", {
          source: r.source ?? "",
          replica: r.replica ?? "",
          proxy: r.proxy ?? "",
        }),
      };
    case "no_keys":
      return {
        tone: "warn",
        text: t(
          r.lake === "replica"
            ? "settings:lake.replication.noKeysReplica"
            : "settings:lake.replication.noKeysMain",
        ),
      };
    case "no_replica":
      return { tone: "warn", text: t("settings:lake.replication.noReplica") };
    default:
      return { tone: "error", text: t("settings:lake.replication.noAddress") };
  }
}

/** A warehouse's state, in a few words, and whether it is in place. */
export function warehouseLine(w: WarehouseReplication): ReplicaLine {
  switch (w.state) {
    case "configured":
      return {
        tone: "ok",
        text: t("settings:lake.replication.configured", {
          at: formatDateTime((w.configured_at ?? w.checked_at) * 1000),
        }),
      };
    case "failed":
      return {
        tone: "error",
        text: t("settings:lake.replication.failed", {
          step: w.step ?? "",
          detail: w.detail ?? "",
        }),
      };
    case "no_keys":
      return { tone: "warn", text: t("settings:lake.replication.noKeys") };
    case "made":
      return { tone: "warn", text: t("settings:lake.replication.made") };
    default:
      return { tone: "warn", text: t("settings:lake.replication.noWarehouse") };
  }
}
