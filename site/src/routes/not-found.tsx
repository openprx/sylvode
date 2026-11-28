import type { JSX } from "@solidjs/web";
import { PageTitle } from "../components/page-title";
import { withBase } from "../lib/base";
import { useSite } from "../lib/site/store";

export default function NotFound(): JSX.Element {
  const site = useSite();
  return (
    <section class="flex flex-col gap-3">
      <PageTitle title={site.t("app.notFound")} />
      <h1 class="text-page font-semibold m-0">{site.t("app.notFound")}</h1>
      <p class="m-0">
        <a href={withBase("/")} class="text-text underline">
          {site.t("app.home")}
        </a>
      </p>
    </section>
  );
}
