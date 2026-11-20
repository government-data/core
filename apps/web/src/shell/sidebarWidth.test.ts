import { describe, expect, it } from "vitest";
import { clampWidth } from "./sidebarWidth";

describe("clampWidth", () => {
  it("keeps the sidebar between its narrowest and its widest", () => {
    expect(clampWidth(100, 1600)).toBe(240);
    expect(clampWidth(455.4, 1600)).toBe(455);
    expect(clampWidth(2000, 1600)).toBe(720);
  });

  it("never takes more than 70% of the window", () => {
    expect(clampWidth(700, 800)).toBe(560);
  });
});
