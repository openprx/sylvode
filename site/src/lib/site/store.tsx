import type { JSX } from "@solidjs/web";
import {
  type Accessor,
  createContext,
  createMemo,
  createSignal,
  onSettled,
  Show,
  useContext,
} from "solid-js";
import { detectLocale, type Locale, type MessageKey, translate } from "../i18n";
import type { Viewer } from "../layout/types";
import { type SiteClient, siteClient } from "./client";
import type { Manifest } from "./types";

export type ManifestStatus = "loading" | "ready" | "error";

export interface SiteContextValue {
  slug: string;
  client: SiteClient;
  locale: Locale;
  t: (key: MessageKey, values?: Record<string, string | number>) => string;
  manifest: Accessor<Manifest | undefined>;
  status: Accessor<ManifestStatus>;
  /** Refetches the manifest (after login or logout the visible pages change). */
  reload: () => Promise<void>;
  viewer: Accessor<Viewer>;
}

const SiteContext = createContext<SiteContextValue>();

export function useSite(): SiteContextValue {
  return useContext(SiteContext);
}

const SLUG_KEY = "sv-site-slug";

/**
 * The site slug: build-time `VITE_SV_SITE_SLUG`, else `?site=<slug>` (remembered for the tab
 * session). The site is chosen by the deployment, never by a path the visitor controls.
 */
export function resolveSlug(): string | null {
  const fixed = import.meta.env.VITE_SV_SITE_SLUG;
  if (fixed) return fixed;
  try {
    const param = new URLSearchParams(window.location.search).get("site");
    if (param && /^[a-z0-9][a-z0-9-]{1,39}$/.test(param)) {
      sessionStorage.setItem(SLUG_KEY, param);
      return param;
    }
    return sessionStorage.getItem(SLUG_KEY);
  } catch {
    return null;
  }
}

export function SiteProvider(props: { slug: string; children?: JSX.Element }): JSX.Element {
  const client = siteClient(props.slug);
  const locale = detectLocale();
  const [manifest, setManifest] = createSignal<Manifest | undefined>(undefined);
  const [status, setStatus] = createSignal<ManifestStatus>("loading");

  const reload = async (): Promise<void> => {
    try {
      const next = await client.manifest();
      setManifest(() => next);
      setStatus("ready");
    } catch {
      setStatus("error");
    }
  };
  onSettled(() => {
    void reload();
  });

  const theme = () => document.documentElement.dataset.theme ?? "light";
  const viewer = createMemo<Viewer>(() => {
    const operator = manifest()?.operator;
    return {
      id: operator?.user_id ?? "",
      role: operator ? operator.role : "site_visitor",
      locale,
      theme: theme(),
    };
  });

  const value: SiteContextValue = {
    slug: props.slug,
    client,
    locale,
    t: (key, values) => translate(locale, key, values),
    manifest,
    status,
    reload,
    viewer,
  };
  return <SiteContext value={value}>{props.children}</SiteContext>;
}

/** Renders `children` once the manifest is available; loading and error states otherwise. */
export function ManifestGate(props: {
  children: (manifest: Manifest) => JSX.Element;
}): JSX.Element {
  const site = useSite();
  return (
    <>
      <Show when={site.status() === "loading" && !site.manifest()}>
        <p class="text-secondary">{site.t("app.loading")}</p>
      </Show>
      <Show when={site.status() === "error" && !site.manifest()}>
        <div class="flex flex-col items-start gap-3" role="alert">
          <p class="m-0">{site.t("app.error")}</p>
          <button
            type="button"
            class="text-text underline cursor-pointer bg-transparent border-0 p-0"
            onClick={() => void site.reload()}
          >
            {site.t("app.retry")}
          </button>
        </div>
      </Show>
      <Show when={site.manifest()}>{(manifest) => props.children(manifest())}</Show>
    </>
  );
}
