import { TextField } from "@kobalte/core/text-field";
import type { ComponentProps, JSX } from "@solidjs/web";
import { omit, Show } from "solid-js";
import { cn } from "./cn";

const control =
  "w-full bg-input text-text text-body border border-input-border rounded-input px-3 placeholder:text-secondary focus-visible:outline-2 focus-visible:outline-focus data-[invalid]:border-danger disabled:opacity-45";

export interface FieldProps extends ComponentProps<typeof TextField> {
  label?: string;
  description?: string;
  errorMessage?: string;
  placeholder?: string;
  type?: string;
}

function Field(props: FieldProps & { multiline: boolean }): JSX.Element {
  const rest = omit(
    props,
    "label",
    "description",
    "errorMessage",
    "placeholder",
    "type",
    "multiline",
    "class",
  );
  return (
    <TextField
      class={cn("flex flex-col gap-1", props.class)}
      validationState={props.errorMessage ? "invalid" : "valid"}
      {...rest}
    >
      <Show when={props.label}>
        <TextField.Label class="text-secondary font-medium">{props.label}</TextField.Label>
      </Show>
      <Show
        when={props.multiline}
        fallback={
          <TextField.Input
            type={props.type}
            placeholder={props.placeholder}
            class={cn(control, "h-9")}
          />
        }
      >
        <TextField.TextArea
          placeholder={props.placeholder}
          class={cn(control, "min-h-24 py-2 resize-y")}
        />
      </Show>
      <Show when={props.description}>
        <TextField.Description class="text-meta text-secondary">
          {props.description}
        </TextField.Description>
      </Show>
      <TextField.ErrorMessage class="text-meta text-danger">
        {props.errorMessage}
      </TextField.ErrorMessage>
    </TextField>
  );
}

export function Input(props: FieldProps): JSX.Element {
  return <Field {...props} multiline={false} />;
}

export function Textarea(props: FieldProps): JSX.Element {
  return <Field {...props} multiline={true} />;
}
