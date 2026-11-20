import { describe, expect, it } from "vitest";
import { updateNotice } from "./updateNotice";

describe("updateNotice", () => {
  it("says that what runs during maintenance waits in the queue", () => {
    const n = updateNotice({ kind: "maintenance", reason: "updating to 1.3.0" });
    expect(n?.tone).toBe("info");
    expect(n?.text).toContain("queda en cola");
    expect(n?.link).toBe(false);
  });

  it("explains how a new version is installed according to the policy", () => {
    const text = (mode: "manual" | "approval" | "automatic", failed = false) =>
      updateNotice({ kind: "available", version: "1.3.0", mode, failed })?.text;
    expect(text("approval")).toContain("1.3.0");
    expect(text("automatic")).toContain("próxima ventana");
    expect(text("manual")).toContain("orquestador actualizar");
    expect(text("automatic", true)).toContain("reintentar");
  });

  it("warns loudly about what went wrong", () => {
    expect(updateNotice({ kind: "critical" })?.tone).toBe("bad");
    const back = updateNotice({ kind: "rolled_back", version: "1.3.0", from: "1.2.0" });
    expect(back?.tone).toBe("warn");
    expect(back?.text).toContain("1.2.0");
  });

  it("shows nothing without a notice", () => {
    expect(updateNotice(null)).toBeNull();
    expect(updateNotice(undefined)).toBeNull();
  });
});
