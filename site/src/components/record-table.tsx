import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";
import { isBlockedType } from "../lib/fields/types";
import type { SiteField, SiteRecord } from "../lib/site/types";
import { FieldValue } from "./field-value";
import { ListFailed, ListSkeleton, type ListStatus } from "./list-state";
import { Table, TBody, TD, TH, THead, TR } from "./ui";

export interface RecordTableProps {
  fields: SiteField[];
  rows: SiteRecord[];
  locale: string;
  empty: string;
  /** `loading` and `failed` never show the empty message; only `ready` with no rows does. */
  status: ListStatus;
  /** Message of the failed state; a generic one when omitted. */
  failedMessage?: string | undefined;
  onRetry?: () => void;
  /** Click on a header sorts by that field (`field`, then `-field`); omitted for a plain list. */
  sort?: string | undefined;
  onSort?: (field: string) => void;
  onRowClick?: (record: SiteRecord) => void;
}

/** A plain list of binding records; blocked field types are never shown. */
export function RecordTable(props: RecordTableProps): JSX.Element {
  const shown = () => props.fields.filter((field) => !isBlockedType(field.type));
  return (
    <Switch>
      <Match when={props.status === "loading"}>
        <ListSkeleton columns={shown().length} />
      </Match>
      <Match when={props.status === "failed"}>
        <ListFailed
          {...(props.failedMessage ? { message: props.failedMessage } : {})}
          {...(props.onRetry ? { onRetry: props.onRetry } : {})}
        />
      </Match>
      <Match when={props.rows.length === 0}>
        <p class="text-secondary m-0" data-list-state="empty">
          {props.empty}
        </p>
      </Match>
      <Match when={true}>
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
                        {props.sort === field.key
                          ? " ▲"
                          : props.sort === `-${field.key}`
                            ? " ▼"
                            : ""}
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
      </Match>
    </Switch>
  );
}
