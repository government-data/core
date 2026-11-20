import type { UpdateNotice } from "./api";
import { tr } from "./i18n";

/** The updates notice in one sentence, with its tone. */
export function updateNotice(
  notice: UpdateNotice | null | undefined,
): { text: string; tone: "info" | "warn" | "bad"; link: boolean } | null {
  switch (notice?.kind) {
    case "maintenance":
      return {
        text: tr("platform:updateNotice.updating", { reason: notice.reason }),
        tone: "info",
        link: false,
      };
    case "critical":
      return {
        text: tr("platform:updateNotice.failedCritical"),
        tone: "bad",
        link: true,
      };
    case "rolled_back":
      return {
        text: tr("platform:updateNotice.rolledBack", {
          version: notice.version,
          from: notice.from,
        }),
        tone: "warn",
        link: true,
      };
    case "available": {
      const how = notice.failed
        ? tr("platform:updateNotice.failedOnce")
        : notice.mode === "automatic"
          ? tr("platform:updateNotice.installsInWindow")
          : notice.mode === "manual"
            ? tr("platform:updateNotice.installByCommand")
            : tr("platform:updateNotice.installWhenever");
      return {
        text: tr("platform:updateNotice.newVersion", { version: notice.version, how: how }),
        tone: "info",
        link: true,
      };
    }
    default:
      return null;
  }
}
