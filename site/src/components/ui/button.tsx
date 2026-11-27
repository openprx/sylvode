import { Button as KButton } from "@kobalte/core/button";
import type { ComponentProps, JSX } from "@solidjs/web";
import { omit } from "solid-js";
import { cn } from "./cn";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";
export type ButtonSize = "sm" | "md";

const base =
  "inline-flex items-center justify-center gap-2 font-medium text-button rounded-button border transition-colors disabled:opacity-45 disabled:pointer-events-none cursor-pointer";

const variants: Record<ButtonVariant, string> = {
  primary: "bg-primary text-on-primary border-primary hover:opacity-90",
  secondary: "bg-surface-white text-text border-line-strong hover:bg-hover",
  ghost: "bg-transparent text-text border-transparent hover:bg-hover",
  danger: "bg-danger text-on-danger border-danger hover:opacity-90",
};

const sizes: Record<ButtonSize, string> = {
  sm: "h-8 px-3",
  md: "h-9 px-4",
};

export interface ButtonProps extends ComponentProps<typeof KButton> {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

export function Button(props: ButtonProps): JSX.Element {
  const rest = omit(props, "variant", "size", "class");
  return (
    <KButton
      class={cn(base, variants[props.variant ?? "primary"], sizes[props.size ?? "md"], props.class)}
      {...rest}
    />
  );
}
