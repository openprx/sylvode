import { browserHistory, createRouter, hashHistory } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { lazy } from "solid-js";
import { ThemeToggle, Toaster } from "./components/ui";

const Home = lazy(() => import("./routes/home"));
const Gallery = lazy(() => import("./routes/gallery"));
const SitePage = lazy(() => import("./routes/page"));
const NotFound = lazy(() => import("./routes/not-found"));

/** `VITE_SV_SITE_ROUTING=hash` lets a plain static file server serve every route. */
const history = import.meta.env.VITE_SV_SITE_ROUTING === "hash" ? hashHistory() : browserHistory();

export const Router = createRouter({
  history,
  routes: [
    { path: "/", component: Home },
    { path: "/gallery", component: Gallery },
    { path: "/p/:slug", component: SitePage },
    { path: "*404", component: NotFound },
  ],
});

function Shell(props: { children?: JSX.Element }): JSX.Element {
  return (
    <div class="min-h-screen flex flex-col">
      <header class="flex items-center justify-between gap-4 px-6 h-14 border-b border-line bg-nav">
        <nav class="flex items-center gap-5 text-button">
          <a href="/" class="font-semibold text-text no-underline">
            Sylvode site
          </a>
          <a href="/gallery" class="text-secondary no-underline hover:text-text">
            Gallery
          </a>
        </nav>
        <ThemeToggle />
      </header>
      <main class="flex-1 w-full max-w-5xl mx-auto px-6 py-8">{props.children}</main>
      <Toaster />
    </div>
  );
}

export function App(): JSX.Element {
  return <Router>{(props) => <Shell>{props.children}</Shell>}</Router>;
}
