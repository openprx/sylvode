import { confirmAction } from "../../components/confirm";
import { showToast } from "../../components/ui";
import type { RuntimeEnv } from "../layout/runtime";
import type { SiteContextValue } from "./store";
import type { Manifest } from "./types";

/** The host environment a layout runtime needs, built from the site context and the router. */
export function createRuntimeEnv(
  site: SiteContextValue,
  manifest: Manifest,
  navigate: (path: string) => void,
  onWritten?: () => void,
): RuntimeEnv {
  return {
    client: site.client,
    manifest,
    viewer: site.viewer(),
    navigate,
    confirm: confirmAction,
    notify: (message, tone) =>
      showToast({
        title: message === "saved" ? site.t("form.saved") : site.t("form.submitted"),
        tone,
      }),
    ...(onWritten ? { onWritten } : {}),
  };
}

export interface RouteMatch {
  page: Manifest["pages"][number];
  params: Record<string, string>;
}

function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const a = pattern.split("/").filter(Boolean);
  const b = path.split("/").filter(Boolean);
  if (a.length !== b.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < a.length; i++) {
    const part = a[i] ?? "";
    const segment = b[i] ?? "";
    if (part.startsWith(":")) params[part.slice(1)] = decodeURIComponent(segment);
    else if (part !== segment) return null;
  }
  return params;
}

/** Finds the manifest page of a path; a static route wins over a parameter route. */
export function matchPage(manifest: Manifest, path: string): RouteMatch | null {
  const normalised = path.length > 1 ? path.replace(/\/+$/, "") : path;
  let best: (RouteMatch & { dynamic: number }) | null = null;
  for (const page of manifest.pages) {
    const params = matchPattern(page.route, normalised);
    if (!params) continue;
    const dynamic = Object.keys(params).length;
    if (!best || dynamic < best.dynamic) best = { page, params, dynamic };
  }
  return best ? { page: best.page, params: best.params } : null;
}
