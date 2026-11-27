import { Checkbox as KCheckbox } from "@kobalte/core/checkbox";
import type { ComponentProps, JSX } from "@solidjs/web";
import { Check } from "lucide";
import { omit, Show } from "solid-js";
import { cn } from "./cn";
import { Icon } from "./icon";

export interface CheckboxProps extends ComponentProps<typeof KCheckbox> {
  label?: string;
}

export function Checkbox(props: CheckboxProps): JSX.Element {
  const rest = omit(props, "label", "class");
  return (
    <KCheckbox class={cn("inline-flex items-center gap-2", props.class)} {...rest}>
      <KCheckbox.Input />
      <KCheckbox.Control class="inline-flex size-5 items-center justify-center rounded-xs border-[1.5px] border-input-border bg-input text-on-primary data-[checked]:bg-primary data-[checked]:border-primary data-[disabled]:opacity-45">
        <KCheckbox.Indicator>
          <Icon icon={Check} size={14} />
        </KCheckbox.Indicator>
      </KCheckbox.Control>
      <Show when={props.label}>
        <KCheckbox.Label class="text-body cursor-pointer">{props.label}</KCheckbox.Label>
      </Show>
    </KCheckbox>
  );
}
