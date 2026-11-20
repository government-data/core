import { sendLanguageToApi, registerLocales } from "@knime-orchestador/i18n";
import en from "./locales/en.json";
import es from "./locales/es.json";

/** The shell's texts (sign-in, session, menu) under `web:`. */
registerLocales("web", { es, en });

/** The server's messages, in the chosen language. */
sendLanguageToApi();
