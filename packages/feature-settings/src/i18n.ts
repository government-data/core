import { registerLocales } from "@knime-orchestador/i18n";
import en from "./locales/en.json";
import es from "./locales/es.json";

/** The Settings texts, under `settings:`. */
registerLocales("settings", { es, en });

export { t, translated } from "@knime-orchestador/i18n";
