import type { JSX } from "@solidjs/web";

export default function Home(): JSX.Element {
  return (
    <section class="flex flex-col gap-3">
      <h1 class="text-page font-semibold m-0">Sylvode site</h1>
      <p class="text-secondary m-0">Front-site skeleton. Pages will be served from the site API.</p>
      <p class="m-0">
        <a href="/gallery" class="text-text underline">
          Open the component gallery
        </a>
      </p>
    </section>
  );
}
