/**
 * Settings → Lake's main lake (decision 0028, "Lake health"; 0066 step 11,
 * the user's ruling 1 of 2026-11-13): today's single RustFS, where the
 * application reaches the lake, and what its admin API answered, asked by
 * the API now with the kept keys. No key reaches this page.
 */

import { t } from "./i18n";
import type { ReplicaLine } from "./lakeReplica";
import type { MainLakeState } from "./machinesApi";

export function mainLine(s: MainLakeState): ReplicaLine {
  const endpoint = s.endpoint ?? "";
  switch (s.state) {
    case "answered":
      return {
        tone: "ok",
        text: t("settings:lake.main.answered", {
          online: s.drives_online ?? 0,
          offline: s.drives_offline ?? 0,
        }),
      };
    case "no_keys":
      return { tone: "warn", text: t("settings:lake.main.noKeys", { endpoint }) };
    case "keys_refused":
      return { tone: "error", text: t("settings:lake.main.keysRefused", { code: s.code ?? "" }) };
    case "unreachable":
      return {
        tone: "error",
        text: t("settings:lake.main.unreachable", { endpoint, detail: s.detail ?? "" }),
      };
    default:
      return { tone: "error", text: t("settings:lake.main.noAddress") };
  }
}
