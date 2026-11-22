import { describe, expect, it } from "vitest";
import type { AllSettings } from "./api";
import { SECTIONS } from "./homeSections";

const unset = { kind: "unset", updated_at: null, updated_by: null } as const;

describe("the platform's card", () => {
  it("shows the installation's time zone", () => {
    const card = SECTIONS.find((s) => s.path === "/settings/platform");
    expect(card).toBeDefined();
    const settings = {
      platform: { value: { timezone: "Europe/Madrid" }, source: unset },
    } as unknown as AllSettings;
    expect(card?.status(settings)).toEqual({ on: true, text: "Europe/Madrid" });
  });
});
