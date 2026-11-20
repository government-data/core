import { describe, expect, it } from "vitest";
import { flatKeys } from "@knime-orchestador/i18n";
import openapi from "../../../../openapi.json";
import { PURPOSES } from "../reveal";
import en from "./en.json";
import es from "./es.json";

describe("quality texts", () => {
  it("Spanish and English have the same keys", () => {
    expect(flatKeys(en).sort()).toEqual(flatKeys(es).sort());
  });

  it("have a text for every purpose of the API's enum, which the dialog offers", () => {
    const keys = openapi.components.schemas.ViewPurpose.enum;
    expect([...PURPOSES].sort()).toEqual([...keys].sort());
    for (const key of keys) {
      expect(en.reveal.purposes).toHaveProperty(key);
      expect(es.reveal.purposes).toHaveProperty(key);
    }
  });
});
