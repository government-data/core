import { describe, expect, it } from "vitest";
import { flatKeys } from "@knime-orchestador/i18n";
import en from "./en.json";
import es from "./es.json";

describe("web texts", () => {
  it("Spanish and English have the same keys", () => {
    expect(flatKeys(en).sort()).toEqual(flatKeys(es).sort());
  });
});
