import { registerLocales } from "@knime-orchestador/i18n";
import en from "./locales/en.json";
import es from "./locales/es.json";

/** The shared components' texts, under `kit:`. */
registerLocales("kit", { es, en });

export { t } from "@knime-orchestador/i18n";
