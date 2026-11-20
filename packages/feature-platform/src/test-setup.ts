import { vi } from "vitest";

// The tests read the Spanish texts: also those that reload the modules (and
// with them the language, which comes from the browser).
vi.stubGlobal("navigator", { ...globalThis.navigator, language: "es-CO", languages: ["es-CO"] });

const { setLang } = await import("@knime-orchestador/i18n");
await import("./i18n");
setLang("es");
