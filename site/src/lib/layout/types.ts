/** Wire types of `sv.layout.v1` as the site renderer reads them (contract C3 section 1). */

export type I18nText = Record<string, string>;
/** A text value: literal, i18n map, reference string, or a computed `{cel}`. */
export type TextValue = string | I18nText | { cel: string } | null | undefined;

export interface LayoutNode {
  type: string;
  props?: Record<string, unknown>;
  children?: LayoutNode[];
  when?: string;
  on_click?: string;
  /** Columns this node spans inside a `grid` parent (1..the grid's columns). */
  col_span?: number;
}

export interface FilterSpec {
  field: string;
  op: "eq" | "neq" | "contains" | "gt" | "gte" | "lt" | "lte" | "not_empty";
  value?: unknown;
}

export type DataSourceSpec =
  | { source: "record"; form?: string; record: string; fields?: string[] }
  | {
      source: "query";
      form: string;
      filter?: FilterSpec[];
      sort?: { field: string; dir?: "asc" | "desc" }[];
      fields?: string[];
      page_size?: number;
    }
  | { source: "form"; form: string; fields?: string[] }
  | { source: "plugin"; [key: string]: unknown };

/** On a site only `submit` and `navigate` with `route` are valid (contract section 1.3). */
export type ActionSpec =
  | {
      kind: "submit";
      data: string;
      mode: "create" | "update";
      record?: string;
      confirm?: TextValue;
    }
  | { kind: "navigate"; to: { route: string } | Record<string, unknown> }
  | { kind: "invoke"; [key: string]: unknown };

export interface LayoutSpec {
  schema: string;
  key: string;
  title?: TextValue;
  params?: Record<string, { type?: "string" | "int" | "bool"; required?: boolean }>;
  data?: Record<string, DataSourceSpec>;
  actions?: Record<string, ActionSpec>;
  root: LayoutNode;
}

export interface Viewer {
  id: string;
  role: string;
  locale: string;
  theme: string;
}

/** What a reference can reach (`$params`, `$data`, `$slot`, `$viewer`, `$actions`). */
export interface RefScope {
  params: Record<string, unknown>;
  data: Record<string, unknown>;
  slot: Record<string, unknown>;
  viewer: Record<string, unknown>;
  actions: Record<string, unknown>;
}
