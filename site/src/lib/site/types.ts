/** Wire types of the site API `/api/site/v1/{slug}` (contract C3 section 6). */

import type { LayoutSpec } from "../layout/types";

export type I18nText = Record<string, string>;
export type SiteAudience = "visitor" | "operator";
export type SiteRole = "operator" | "editor";

export interface SiteInfo {
  slug: string;
  title: string | I18nText;
  theme?: string | null;
}

export interface SiteOperator {
  user_id: string;
  display_name: string;
  role: SiteRole;
}

export interface ManifestPage {
  route: string;
  audience: SiteAudience;
  /** Navigation entry; null hides the page from the menu. Shape `{label?, position?}` or a label. */
  nav: { label?: string | I18nText; position?: number } | string | I18nText | null;
  layout_key: string;
}

export interface ManifestBinding {
  key: string;
  visitor: { read_fields: string[]; submit_fields: string[] };
  operator: { read_fields: string[]; update_fields: string[] } | null;
}

export interface Manifest {
  site: SiteInfo;
  operator: SiteOperator | null;
  pages: ManifestPage[];
  bindings: ManifestBinding[];
}

export interface PageResponse {
  route: string;
  audience: SiteAudience;
  params: Record<string, string>;
  layout: LayoutSpec;
}

export interface SiteField {
  key: string;
  label: string;
  type: string;
  required?: boolean;
  options?: string[];
  /** Client-side rule definitions (C2 mirror); optional hints, the server decides. */
  rules_client?: unknown[];
}

export interface FieldsResponse {
  fields: SiteField[];
}

export interface SiteRecord {
  id: string;
  values: Record<string, unknown>;
  title?: string | null;
  created_at: string;
  updated_at: string;
}

export interface Paginated<T> {
  items: T[];
  total: number;
  page: number;
  per_page: number;
  total_pages?: number;
}

export interface ListQuery {
  page?: number;
  per_page?: number;
  /** `filter[<field>]=<value>` pairs. */
  filter?: Record<string, string>;
  /** `<field>` or `-<field>`. */
  sort?: string;
}

export interface SubmitResult {
  id: string;
  values: Record<string, unknown>;
}

export interface UpdateResult extends SubmitResult {
  updated_at: string;
}

export interface LoginResult {
  operator: SiteOperator;
}

/** One entry of a `validation_failed` report (C2); both `field` and a JSON-pointer `path` occur. */
export interface FieldIssue {
  field?: string;
  path?: string;
  code: string;
  message?: string;
}
