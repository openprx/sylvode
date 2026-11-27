import { render } from "@solidjs/web";
import { App } from "./app";
import "./styles/app.css";

async function start(): Promise<void> {
  // The mock backend is a build-time opt-in: without VITE_SV_SITE_MOCK=1 this branch and its
  // fixtures are removed from the bundle.
  if (import.meta.env.VITE_SV_SITE_MOCK === "1") {
    const mock = await import("./lib/site/mock");
    mock.installMock();
  }
  const root = document.getElementById("app");
  if (!root) {
    throw new Error("missing #app mount element");
  }
  render(() => <App />, root);
}

void start();
