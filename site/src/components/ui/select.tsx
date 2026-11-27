import { Select as KSelect } from "@kobalte/core/select";
import type { JSX } from "@solidjs/web";
import { Check, ChevronDown } from "lucide";
import { Show } from "solid-js";
import { cn } from "./cn";
import { Icon } from "./icon";

export interface SelectOption {
  value: string;
  label: string;
}

export interface SelectProps {
  options: SelectOption[];
  value?: SelectOption;
  onChange?: (value: SelectOption | null) => void;
  label?: string;
  placeholder?: string;
  class?: string;
}

export function Select(props: SelectProps): JSX.Element {
  return (
    <KSelect<SelectOption>
      class={cn("flex flex-col gap-1", props.class)}
      options={props.options}
      optionValue="value"
      optionTextValue="label"
      value={props.value}
      onChange={props.onChange}
      placeholder={props.placeholder}
      itemComponent={(item) => (
        <KSelect.Item
          item={item.item}
          class="flex items-center justify-between h-9 px-3 rounded-sm text-body cursor-pointer data-[highlighted]:bg-hover outline-none"
        >
          <KSelect.ItemLabel>{item.item.rawValue.label}</KSelect.ItemLabel>
          <KSelect.ItemIndicator>
            <Icon icon={Check} />
          </KSelect.ItemIndicator>
        </KSelect.Item>
      )}
    >
      <Show when={props.label}>
        <KSelect.Label class="text-secondary font-medium">{props.label}</KSelect.Label>
      </Show>
      <KSelect.Trigger class="flex h-9 w-full items-center justify-between gap-2 bg-input text-text text-body border border-input-border rounded-input px-3 cursor-pointer">
        <KSelect.Value<SelectOption>>{(state) => state.selectedOption()?.label}</KSelect.Value>
        <KSelect.Icon>
          <Icon icon={ChevronDown} />
        </KSelect.Icon>
      </KSelect.Trigger>
      <KSelect.Portal>
        <KSelect.Content class="z-50 bg-overlay border border-overlay-border rounded-md shadow-overlay p-1 min-w-40">
          <KSelect.Listbox class="outline-none" />
        </KSelect.Content>
      </KSelect.Portal>
    </KSelect>
  );
}
