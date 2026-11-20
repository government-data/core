import { registerLocales } from "@knime-orchestador/i18n";
import en from "./locales/en.json";
import es from "./locales/es.json";

/** The platform texts (license, updates, notices…), under `platform:`. */
registerLocales("platform", { es, en });

export { t as tr, translated } from "@knime-orchestador/i18n";
