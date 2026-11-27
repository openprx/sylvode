import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { isBlockedType } from "../lib/fields/types";
import { formatValue } from "../lib/layout/format";
import { displayValue } from "../lib/layout/refs";
import { Badge } from "./ui";

export interface FieldValueProps {
  type: string;
  value: unknown;
  locale: string;
}

/** Read-only presentation of one value by field type. Always text, never HTML. */
export function FieldValue(props: FieldValueProps): JSX.Element {
  return (
    <Show when={!isBlockedType(props.type)} fallback={<span class="text-secondary">-</span>}>
      <Show
        when={props.type !== "boolean"}
        fallback={<span>{props.value === true ? "✓" : props.value === false ? "-" : ""}</span>}
      >
        <Show
          when={props.type !== "multi_select" || !Array.isArray(props.value)}
          fallback={
            <span class="inline-flex flex-wrap gap-1">
              {(props.value as unknown[]).map((item) => (
                <Badge>{displayValue(item)}</Badge>
              ))}
            </span>
          }
        >
          <span class="break-words whitespace-pre-wrap">{textOf(props)}</span>
        </Show>
      </Show>
    </Show>
  );
}

function textOf(props: FieldValueProps): string {
  switch (props.type) {
    case "amount":
      return formatValue(props.value, "amount", props.locale);
    case "number":
    case "integer":
      return formatValue(props.value, "number", props.locale);
    case "progress":
      return formatValue(props.value, "percent", props.locale);
    case "date":
      return formatValue(props.value, "date", props.locale);
    case "datetime":
      return formatValue(props.value, "datetime", props.locale);
    case "rating":
      return props.value === null || props.value === undefined || props.value === ""
        ? ""
        : `${displayValue(props.value)} / 5`;
    default:
      return displayValue(props.value);
  }
}
