import type { JSX } from "@solidjs/web";

export default function NotFound(): JSX.Element {
  return (
    <section class="flex flex-col gap-3">
      <h1 class="text-page font-semibold m-0">Page not found</h1>
      <p class="m-0">
        <a href="/" class="text-text underline">
          Back to the home page
        </a>
      </p>
    </section>
  );
}
