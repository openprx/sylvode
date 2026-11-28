import type { JSX } from "@solidjs/web";
import { withBase } from "../lib/base";
import { useSite } from "../lib/site/store";

export default function NotFound(): JSX.Element {
  const site = useSite();
  return (
    <section class="flex flex-col gap-3">
      <h1 class="text-page font-semibold m-0">{site.t("app.notFound")}</h1>
      <p class="m-0">
        <a href={withBase("/")} class="text-text underline">
          {site.t("app.home")}
        </a>
      </p>
    </section>
  );
}
