import { describe, expect, it } from "vitest";
import { statusIcon, type Tone } from "./status";

describe("statusIcon", () => {
  it("gives every tone an icon of its own, so color never carries it alone", () => {
    const tones: Tone[] = ["good", "warning", "serious", "critical", "neutral"];
    const icons = tones.map(statusIcon);
    expect(new Set(icons).size).toBe(tones.length);
    expect(icons.every((i) => i.length > 0)).toBe(true);
  });
});
