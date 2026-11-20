import { describe, expect, it } from "vitest";
import { flatKeys, lang, pickLang, registerLocales, setLang, t, translated } from "./index";

describe("i18n", () => {
  it("picks the browser's language, or Spanish", () => {
    expect(pickLang(["en-US", "es"])).toBe("en");
    expect(pickLang(["es-CO"])).toBe("es");
    expect(pickLang(["fr-FR", "de"])).toBe("es");
    expect(pickLang([])).toBe("es");
  });

  it("translates, interpolates, pluralizes and falls back to Spanish", () => {
    registerLocales("test", {
      es: {
        hello: "Hola, {{name}}",
        workflows_one: "{{count}} flujo",
        workflows_other: "{{count}} flujos",
        only: "Solo en español",
      },
      en: {
        hello: "Hi, {{name}}",
        workflows_one: "{{count}} workflow",
        workflows_other: "{{count}} workflows",
      },
    });
    setLang("en");
    expect(lang()).toBe("en");
    expect(t("test:hello", { name: "Ana" })).toBe("Hi, Ana");
    expect(t("test:workflows", { count: 1 })).toBe("1 workflow");
    expect(t("test:workflows", { count: 3 })).toBe("3 workflows");
    expect(t("test:only")).toBe("Solo en español");
    setLang("es");
    expect(t("test:workflows", { count: 3 })).toBe("3 flujos");
  });

  it("translated rebuilds a module's labels when the language changes", () => {
    registerLocales("test-tr", { es: { hello: "Hola" }, en: { hello: "Hello" } });
    setLang("es");
    const labels = translated(() => ({ greet: t("test-tr:hello") }));
    const options = translated(() => [{ label: t("test-tr:hello") }]);
    expect(labels.greet).toBe("Hola");
    expect(Object.keys(labels)).toEqual(["greet"]);
    setLang("en");
    expect(labels.greet).toBe("Hello");
    expect(Array.isArray(options)).toBe(true);
    expect(options.map((o) => o.label)).toEqual(["Hello"]);
    expect(options.length).toBe(1);
    setLang("es");
  });

  it("flattens the keys to compare dictionaries", () => {
    expect(flatKeys({ a: "x", b: { c: "y", d: { e: "z" } } })).toEqual(["a", "b.c", "b.d.e"]);
  });
});
