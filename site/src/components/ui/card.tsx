import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { cn } from "./cn";

export interface CardProps {
  title?: string;
  description?: string;
  class?: string;
  children?: JSX.Element;
  footer?: JSX.Element;
}

export function Card(props: CardProps): JSX.Element {
  return (
    <section
      class={cn(
        "bg-surface-white border border-line rounded-card p-5 flex flex-col gap-3",
        props.class,
      )}
    >
      <Show when={props.title}>
        <header class="flex flex-col gap-1">
          <h3 class="text-subheading font-semibold m-0">{props.title}</h3>
          <Show when={props.description}>
            <p class="text-secondary text-small m-0">{props.description}</p>
          </Show>
        </header>
      </Show>
      {props.children}
      <Show when={props.footer}>
        <footer class="flex items-center gap-2 pt-2 border-t border-line">{props.footer}</footer>
      </Show>
    </section>
  );
}
