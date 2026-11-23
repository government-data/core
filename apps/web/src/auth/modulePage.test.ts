import { describe, expect, it } from "vitest";
import { isModulePage } from "./api";

describe("a module's menu entry", () => {
  const entry = (url: string, kind: "module" | "app" = "module") => ({
    kind,
    name: "User guide",
    detail: "",
    url,
  });

  it("is a path of this site", () => {
    expect(isModulePage(entry("/docs/"))).toBe(true);
    expect(isModulePage(entry("/docs/es/"))).toBe(true);
  });

  it("never another site, nor an entry that is not a module", () => {
    expect(isModulePage(entry("//evil.test/"))).toBe(false);
    expect(isModulePage(entry("https://evil.test/"))).toBe(false);
    expect(isModulePage(entry("/\\evil.test"))).toBe(false);
    expect(isModulePage(entry("/docs/", "app"))).toBe(false);
  });
});
