import { render } from "@solidjs/web";
import { App } from "./app";
import { detectLocale } from "./lib/i18n";
import "./styles/app.css";

async function start(): Promise<void> {
  // The mock backend is a build-time opt-in: without VITE_SV_SITE_MOCK=1 this branch and its
  // fixtures are removed from the bundle.
  if (import.meta.env.VITE_SV_SITE_MOCK === "1") {
    const mock = await import("./lib/site/mock");
    mock.installMock();
  }
  // WCAG 3.1.1: the document language follows the active locale, not the static index.html.
  document.documentElement.lang = detectLocale() === "zh" ? "zh-CN" : "en";
  const root = document.getElementById("app");
  if (!root) {
    throw new Error("missing #app mount element");
  }
  render(() => <App />, root);
}

void start();
