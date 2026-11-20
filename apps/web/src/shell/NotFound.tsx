import { A } from "@solidjs/router";
import type { Component } from "solid-js";
import { t } from "@knime-orchestador/i18n";

/** Fallback view for a URL no registered feature owns. */
export const NotFound: Component = () => {
  return (
    <section class="feature">
      <header class="feature__header">
        <h1 class="feature__title">{t("web:shell.notFound.title")}</h1>
        <p class="feature__subtitle">{t("web:shell.notFound.body")}</p>
      </header>
      <A class="link" href="/">
        {t("web:shell.backHome")}
      </A>
    </section>
  );
};
