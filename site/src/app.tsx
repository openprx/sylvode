import { browserHistory, createRouter, hashHistory } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { For, lazy, Show } from "solid-js";
import { ConfirmHost } from "./components/confirm";
import { Button, ThemeToggle, Toaster } from "./components/ui";
import { pickText } from "./lib/i18n";
import { resolveSlug, SiteProvider, useSite } from "./lib/site/store";
import type { ManifestPage } from "./lib/site/types";

const Home = lazy(() => import("./routes/home"));
const Gallery = lazy(() => import("./routes/gallery"));
const SitePage = lazy(() => import("./routes/page"));
const Login = lazy(() => import("./routes/login"));
const Mine = lazy(() => import("./routes/mine"));
const Operator = lazy(() => import("./routes/operator"));

/** `VITE_SV_SITE_ROUTING=hash` lets a plain static file server serve every route. */
const history = import.meta.env.VITE_SV_SITE_ROUTING === "hash" ? hashHistory() : browserHistory();

/**
 * The app's own routes come first; every other path is a page of the site manifest (resolved in
 * `routes/page.tsx` from the manifest, so the route table needs no rebuild when pages change).
 */
export const Router = createRouter({
  history,
  routes: [
    { path: "/", component: Home },
    { path: "/gallery", component: Gallery },
    { path: "/login", component: Login },
    { path: "/mine/:binding", component: Mine },
    { path: "/operator/:binding", component: Operator },
    { path: "/p/:slug", component: SitePage },
    { path: "*404", component: SitePage },
  ],
});

function navLabel(page: ManifestPage, locale: string): string | null {
  const nav = page.nav;
  if (nav === null || nav === undefined) return null;
  if (typeof nav === "string") return nav;
  if ("label" in nav || "position" in nav) {
    const label = (nav as { label?: string | Record<string, string> }).label;
    return label ? pickText(label, locale) : page.route;
  }
  return pickText(nav as Record<string, string>, locale);
}

function navPosition(page: ManifestPage): number {
  const nav = page.nav;
  return nav && typeof nav === "object" && "position" in nav ? Number(nav.position ?? 0) : 0;
}

function Header(): JSX.Element {
  const site = useSite();
  const links = () => {
    const manifest = site.manifest();
    if (!manifest) return [];
    return manifest.pages
      .map((page) => ({ page, label: navLabel(page, site.locale) }))
      .filter((entry): entry is { page: ManifestPage; label: string } => entry.label !== null)
      .sort((a, b) => navPosition(a.page) - navPosition(b.page));
  };
  const mine = () =>
    (site.manifest()?.bindings ?? []).filter((binding) => binding.visitor.read_fields.length > 0);
  const operatorBindings = () =>
    (site.manifest()?.operator ? (site.manifest()?.bindings ?? []) : []).filter(
      (binding) => binding.operator,
    );
  const logout = async () => {
    try {
      await site.client.logout();
    } finally {
      await site.reload();
      window.location.assign("/");
    }
  };
  return (
    <header class="flex flex-wrap items-center justify-between gap-4 px-6 min-h-14 border-b border-line bg-nav">
      <nav class="flex flex-wrap items-center gap-5 text-button">
        <a href="/" class="font-semibold text-text no-underline">
          {pickText(site.manifest()?.site.title, site.locale) || "Sylvode"}
        </a>
        <For each={links()}>
          {(entry) => (
            <a href={entry.page.route} class="text-secondary no-underline hover:text-text">
              {entry.label}
            </a>
          )}
        </For>
        <For each={mine()}>
          {(binding) => (
            <a href={`/mine/${binding.key}`} class="text-secondary no-underline hover:text-text">
              {site.t("nav.mine")}
            </a>
          )}
        </For>
        <For each={operatorBindings()}>
          {(binding) => (
            <a
              href={`/operator/${binding.key}`}
              class="text-secondary no-underline hover:text-text"
            >
              {site.t("nav.operator")}
            </a>
          )}
        </For>
      </nav>
      <div class="flex items-center gap-3">
        <Show when={import.meta.env.VITE_SV_SITE_MOCK === "1"}>
          <span class="text-meta text-warning bg-warning-bg rounded-chip px-2" data-mock-banner>
            {site.t("app.mock")}
          </span>
        </Show>
        <Show
          when={site.manifest()?.operator}
          fallback={
            <a href="/login" class="text-secondary text-button no-underline hover:text-text">
              {site.t("nav.login")}
            </a>
          }
        >
          {(operator) => (
            <span class="flex items-center gap-2 text-small">
              {operator().display_name}
              <Button variant="ghost" size="sm" onClick={() => void logout()}>
                {site.t("nav.logout")}
              </Button>
            </span>
          )}
        </Show>
        <ThemeToggle />
      </div>
    </header>
  );
}

function Shell(props: { children?: JSX.Element }): JSX.Element {
  const slug = resolveSlug();
  return (
    <div class="min-h-screen flex flex-col">
      <Show
        when={slug}
        fallback={
          <main class="flex-1 w-full max-w-5xl mx-auto px-6 py-8">
            <p>
              No site is configured. Set VITE_SV_SITE_SLUG or open the page with ?site=&lt;slug&gt;.
            </p>
          </main>
        }
      >
        {(value) => (
          <SiteProvider slug={value()}>
            <Header />
            <main class="flex-1 w-full max-w-5xl mx-auto px-6 py-8">{props.children}</main>
            <ConfirmHost confirmLabel="OK" cancelLabel="Cancel" />
          </SiteProvider>
        )}
      </Show>
      <Toaster />
    </div>
  );
}

export function App(): JSX.Element {
  return <Router>{(props) => <Shell>{props.children}</Shell>}</Router>;
}
