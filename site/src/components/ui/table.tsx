import type { JSX } from "@solidjs/web";
import { cn } from "./cn";

export function Table(props: { class?: string; children?: JSX.Element }): JSX.Element {
  return (
    <div class="w-full overflow-x-auto border border-line rounded-card bg-surface-white">
      <table class={cn("w-full border-collapse text-small text-left", props.class)}>
        {props.children}
      </table>
    </div>
  );
}

export function THead(props: { children?: JSX.Element }): JSX.Element {
  return <thead class="bg-surface text-secondary">{props.children}</thead>;
}

export function TBody(props: { children?: JSX.Element }): JSX.Element {
  return <tbody>{props.children}</tbody>;
}

export function TR(props: { children?: JSX.Element }): JSX.Element {
  return <tr class="border-b border-line last:border-b-0 hover:bg-hover">{props.children}</tr>;
}

export function TH(props: { class?: string; children?: JSX.Element }): JSX.Element {
  return <th class={cn("px-4 h-10 font-medium", props.class)}>{props.children}</th>;
}

export function TD(props: { class?: string; children?: JSX.Element }): JSX.Element {
  return <td class={cn("px-4 h-11", props.class)}>{props.children}</td>;
}
