/**
 * Settings → Lake's replica (decision 0028, "Lake health"; 0066 step 11):
 * which machine it is and what its admin API answered, asked by the API
 * now with the kept keys. No key reaches this page.
 */

import { t } from "./i18n";
import type { ReplicaState } from "./machinesApi";

/** One line for the page: how it reads, and what it says. */
export type ReplicaLine = { tone: "ok" | "warn" | "error"; text: string };

export function replicaLine(s: ReplicaState): ReplicaLine {
  const machine = s.machine ?? "";
  switch (s.state) {
    case "answered":
      return {
        tone: "ok",
        text: t("settings:lake.replica.answered", {
          machine,
          online: s.drives_online ?? 0,
          offline: s.drives_offline ?? 0,
        }),
      };
    case "no_keys":
      return { tone: "warn", text: t("settings:lake.replica.noKeys", { machine }) };
    case "keys_refused":
      return {
        tone: "error",
        text: t("settings:lake.replica.keysRefused", { machine, code: s.code ?? "" }),
      };
    case "unreachable":
      return {
        tone: "error",
        text: t("settings:lake.replica.unreachable", { machine, detail: s.detail ?? "" }),
      };
    case "several_machines":
      return {
        tone: "error",
        text: t("settings:lake.replica.several", { machines: s.machines.join(", ") }),
      };
    default:
      return s.pending
        ? { tone: "warn", text: t("settings:lake.replica.pending", { machine: s.pending }) }
        : { tone: "warn", text: t("settings:lake.replica.noMachine") };
  }
}
