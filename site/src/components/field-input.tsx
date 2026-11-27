import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { inputKind } from "../lib/fields/types";
import type { SiteField } from "../lib/site/types";
import { cn } from "./ui/cn";

const control =
  "w-full bg-input text-text text-body border border-input-border rounded-input px-3 h-9 placeholder:text-secondary focus-visible:outline-2 focus-visible:outline-focus disabled:opacity-45";

export interface FieldInputProps {
  field: SiteField;
  value: unknown;
  onChange: (value: unknown) => void;
  error?: string | undefined;
  disabled?: boolean;
  /** Form wide id prefix so labels stay unique when several forms are on one page. */
  idPrefix?: string;
}

function asString(value: unknown): string {
  return value === null || value === undefined ? "" : String(value);
}

/** `datetime-local` wants `YYYY-MM-DDTHH:mm`; the wire value is RFC 3339. */
function toLocalInput(value: unknown): string {
  const text = asString(value);
  if (!text) return "";
  const date = new Date(text);
  if (Number.isNaN(date.getTime())) return text;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fromLocalInput(text: string): string {
  if (!text) return "";
  const date = new Date(text);
  return Number.isNaN(date.getTime()) ? text : date.toISOString();
}

/** Public editor of one field; blocked types render nothing (C2 outlets `Reject`). */
export function FieldInput(props: FieldInputProps): JSX.Element {
  const kind = () => inputKind(props.field.type);
  const id = () => `${props.idPrefix ?? "f"}-${props.field.key}`;
  const invalid = () => (props.error ? "true" : undefined);
  const options = () => props.field.options ?? [];
  return (
    <Show when={kind()}>
      <div
        class="flex flex-col gap-1"
        data-field={props.field.key}
        data-field-type={props.field.type}
      >
        <Show
          when={kind() === "boolean"}
          fallback={
            <label for={id()} class="text-secondary font-medium">
              {props.field.label}
              {props.field.required ? " *" : ""}
            </label>
          }
        >
          <span class="sr-only">{props.field.label}</span>
        </Show>
        <Show when={kind() === "text"}>
          <input
            id={id()}
            class={control}
            type={
              props.field.type === "email" ? "email" : props.field.type === "phone" ? "tel" : "text"
            }
            value={asString(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </Show>
        <Show when={kind() === "textarea"}>
          <textarea
            id={id()}
            class={cn(control, "min-h-24 py-2 h-auto resize-y")}
            value={asString(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </Show>
        <Show when={kind() === "number"}>
          <input
            id={id()}
            class={control}
            type="number"
            step={props.field.type === "number" || props.field.type === "amount" ? "any" : "1"}
            value={asString(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onInput={(event) => {
              const raw = event.currentTarget.value;
              if (raw === "") return props.onChange(undefined);
              const n = Number(raw);
              props.onChange(Number.isFinite(n) ? n : raw);
            }}
          />
        </Show>
        <Show when={kind() === "date"}>
          <input
            id={id()}
            class={control}
            type="date"
            value={asString(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onInput={(event) => props.onChange(event.currentTarget.value)}
          />
        </Show>
        <Show when={kind() === "datetime"}>
          <input
            id={id()}
            class={control}
            type="datetime-local"
            value={toLocalInput(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onInput={(event) => props.onChange(fromLocalInput(event.currentTarget.value))}
          />
        </Show>
        <Show when={kind() === "single_select"}>
          <select
            id={id()}
            class={control}
            value={asString(props.value)}
            disabled={props.disabled}
            aria-invalid={invalid()}
            onChange={(event) => props.onChange(event.currentTarget.value)}
          >
            <option value="" />
            <For each={options()}>{(option) => <option value={option}>{option}</option>}</For>
          </select>
        </Show>
        <Show when={kind() === "multi_select"}>
          <fieldset
            id={id()}
            class="flex flex-wrap gap-3 border-0 p-0 m-0"
            aria-label={props.field.label}
          >
            <For each={options()}>
              {(option) => (
                <label class="inline-flex items-center gap-2 text-body">
                  <input
                    type="checkbox"
                    checked={Array.isArray(props.value) && props.value.includes(option)}
                    disabled={props.disabled}
                    onChange={(event) => {
                      const current = Array.isArray(props.value) ? (props.value as string[]) : [];
                      props.onChange(
                        event.currentTarget.checked
                          ? [...current, option]
                          : current.filter((item) => item !== option),
                      );
                    }}
                  />
                  {option}
                </label>
              )}
            </For>
          </fieldset>
        </Show>
        <Show when={kind() === "boolean"}>
          <label class="inline-flex items-center gap-2 text-body">
            <input
              id={id()}
              type="checkbox"
              checked={props.value === true}
              disabled={props.disabled}
              onChange={(event) => props.onChange(event.currentTarget.checked)}
            />
            {props.field.label}
            {props.field.required ? " *" : ""}
          </label>
        </Show>
        <Show when={props.error}>
          <p class="text-meta text-danger m-0" role="alert">
            {props.error}
          </p>
        </Show>
      </div>
    </Show>
  );
}
