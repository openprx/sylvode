import type { JSX } from "@solidjs/web";
import { Dynamic } from "@solidjs/web";
import { For } from "solid-js";
import { cn } from "./cn";

type IconNode = [tag: string, attrs: Record<string, string | number | undefined>][];

export interface IconProps {
  /** A node exported by the `lucide` package, for example `Sun`. */
  icon: IconNode;
  size?: number;
  class?: string;
  label?: string;
}

export function Icon(props: IconProps): JSX.Element {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={props.size ?? 16}
      height={props.size ?? 16}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      class={cn("shrink-0", props.class)}
      aria-hidden={props.label ? undefined : "true"}
      aria-label={props.label}
      role={props.label ? "img" : undefined}
    >
      <For each={props.icon}>{(node) => <Dynamic component={node[0]} {...node[1]} />}</For>
    </svg>
  );
}
