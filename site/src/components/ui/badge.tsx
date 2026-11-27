import type { JSX } from "@solidjs/web";
import { cn } from "./cn";

export type BadgeTone = "neutral" | "success" | "warning" | "danger" | "solid";

const tones: Record<BadgeTone, string> = {
  neutral: "bg-surface text-text",
  success: "bg-success-bg text-success",
  warning: "bg-warning-bg text-warning",
  danger: "bg-danger-bg text-danger",
  solid: "bg-badge text-on-badge",
};

export interface BadgeProps {
  tone?: BadgeTone;
  class?: string;
  children?: JSX.Element;
}

export function Badge(props: BadgeProps): JSX.Element {
  return (
    <span
      class={cn(
        "inline-flex items-center rounded-chip px-2 h-5 text-meta font-medium",
        tones[props.tone ?? "neutral"],
        props.class,
      )}
    >
      {props.children}
    </span>
  );
}
