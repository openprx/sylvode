import type { JSX } from "@solidjs/web";
import { createEffect } from "solid-js";
import { pickText } from "../lib/i18n";
import { useSite } from "../lib/site/store";

/**
 * Sets `document.title` to `<page> · <site>` (or the site title alone) while mounted (WCAG 2.4.2).
 * Renders nothing; the previous title is restored by the next page that mounts.
 */
export function PageTitle(props: { title?: string | undefined }): JSX.Element {
  const site = useSite();
  createEffect(
    () => ({
      page: props.title?.trim() ?? "",
      site: pickText(site.manifest()?.site.title, site.locale),
    }),
    ({ page, site: siteTitle }) => {
      document.title =
        page && siteTitle && page !== siteTitle
          ? `${page} · ${siteTitle}`
          : page || siteTitle || "Sylvode";
    },
  );
  return null;
}
