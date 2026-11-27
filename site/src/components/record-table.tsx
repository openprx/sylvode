import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { isBlockedType } from "../lib/fields/types";
import type { SiteField, SiteRecord } from "../lib/site/types";
import { FieldValue } from "./field-value";
import { Table, TBody, TD, TH, THead, TR } from "./ui";

export interface RecordTableProps {
  fields: SiteField[];
  rows: SiteRecord[];
  locale: string;
  empty: string;
  /** Click on a header sorts by that field (`field`, then `-field`); omitted for a plain list. */
  sort?: string | undefined;
  onSort?: (field: string) => void;
  onRowClick?: (record: SiteRecord) => void;
}

/** A plain list of binding records; blocked field types are never shown. */
export function RecordTable(props: RecordTableProps): JSX.Element {
  const shown = () => props.fields.filter((field) => !isBlockedType(field.type));
  return (
    <Show when={props.rows.length > 0} fallback={<p class="text-secondary m-0">{props.empty}</p>}>
      <Table>
        <THead>
          <tr>
            <For each={shown()}>
              {(field) => (
                <TH>
                  <Show when={props.onSort} fallback={field.label}>
                    <button
                      type="button"
                      class="bg-transparent border-0 p-0 font-medium text-secondary cursor-pointer hover:text-text"
                      onClick={() => props.onSort?.(field.key)}
                    >
                      {field.label}
                      {props.sort === field.key ? " ▲" : props.sort === `-${field.key}` ? " ▼" : ""}
                    </button>
                  </Show>
                </TH>
              )}
            </For>
          </tr>
        </THead>
        <TBody>
          <For each={props.rows}>
            {(row) => (
              <TR>
                <For each={shown()}>
                  {(field) => (
                    <TD>
                      <Show
                        when={props.onRowClick}
                        fallback={
                          <FieldValue
                            type={field.type}
                            value={row.values[field.key]}
                            locale={props.locale}
                          />
                        }
                      >
                        <button
                          type="button"
                          class="bg-transparent border-0 p-0 text-left text-text cursor-pointer"
                          data-record-id={row.id}
                          onClick={() => props.onRowClick?.(row)}
                        >
                          <FieldValue
                            type={field.type}
                            value={row.values[field.key]}
                            locale={props.locale}
                          />
                        </button>
                      </Show>
                    </TD>
                  )}
                </For>
              </TR>
            )}
          </For>
        </TBody>
      </Table>
    </Show>
  );
}
