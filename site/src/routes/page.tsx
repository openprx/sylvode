import { useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";

export default function SitePage(): JSX.Element {
  const params = useParams();
  return (
    <section class="flex flex-col gap-3">
      <h1 class="text-page font-semibold m-0">Page: {params.slug}</h1>
      <p class="text-secondary m-0">
        Placeholder for a site page; content loading is not built yet.
      </p>
    </section>
  );
}
