/**
 * Deployment base of the app, from Vite's `base` (`VITE_SV_SITE_BASE`, default `/site/`).
 * Hash routing keeps every route inside the hash, so the router base is empty there.
 */
export const isHashRouting = import.meta.env.VITE_SV_SITE_ROUTING === "hash";

/** Router base without a trailing slash (`/site`); empty for hash routing or base `/`. */
export const routerBase: string = isHashRouting ? "" : import.meta.env.BASE_URL.replace(/\/+$/, "");

/** Prefix an app route (`/gallery`) with the base for use in `href` and full-page navigation. */
export function withBase(route: string): string {
  return `${routerBase}${route.startsWith("/") ? route : `/${route}`}`;
}

/** Remove the base from a router pathname so it can be compared with manifest routes. */
export function stripBase(pathname: string): string {
  if (routerBase && (pathname === routerBase || pathname.startsWith(`${routerBase}/`))) {
    return pathname.slice(routerBase.length) || "/";
  }
  return pathname || "/";
}
