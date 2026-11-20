/* @refresh reload */
import "./i18n";
import { render } from "solid-js/web";
import { App } from "./App";
import { initTheme } from "./shell/theme";
import "./shell/theme.css";
import "./shell/admin.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("Root element #root not found");
}

// The remembered theme, before the app paints.
initTheme();
render(() => <App />, container);
