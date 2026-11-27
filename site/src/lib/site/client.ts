import { ApiError, api } from "../api";
import type {
  FieldIssue,
  FieldsResponse,
  ListQuery,
  LoginResult,
  Manifest,
  PageResponse,
  Paginated,
  SiteRecord,
  SubmitResult,
  UpdateResult,
} from "./types";

/** Typed client of every `/api/site/v1/{slug}` endpoint (contract C3 section 6.6). */
export function siteClient(slug: string) {
  const base = `/${encodeURIComponent(slug)}`;
  const binding = (key: string) => `${base}/bindings/${encodeURIComponent(key)}`;
  return {
    manifest: (signal?: AbortSignal) =>
      api.get<Manifest>(`${base}/manifest`, signal ? { signal } : {}),
    page: (route: string, signal?: AbortSignal) =>
      api.get<PageResponse>(`${base}/pages`, { query: { route }, ...(signal ? { signal } : {}) }),
    fields: (key: string, signal?: AbortSignal) =>
      api.get<FieldsResponse>(`${binding(key)}/fields`, signal ? { signal } : {}),
    listRecords: (key: string, query: ListQuery = {}, signal?: AbortSignal) => {
      const q: Record<string, string | number> = {};
      if (query.page !== undefined) q.page = query.page;
      if (query.per_page !== undefined) q.per_page = Math.min(query.per_page, 100);
      if (query.sort) q.sort = query.sort;
      for (const [field, value] of Object.entries(query.filter ?? {})) {
        q[`filter[${field}]`] = value;
      }
      return api.get<Paginated<SiteRecord>>(`${binding(key)}/records`, {
        query: q,
        ...(signal ? { signal } : {}),
      });
    },
    getRecord: (key: string, id: string, signal?: AbortSignal) =>
      api.get<SiteRecord>(`${binding(key)}/records/${encodeURIComponent(id)}`, {
        ...(signal ? { signal } : {}),
      }),
    submit: (key: string, values: Record<string, unknown>) =>
      api.post<SubmitResult>(`${binding(key)}/records`, { body: { values } }),
    update: (key: string, id: string, values: Record<string, unknown>, expectedUpdatedAt: string) =>
      api.patch<UpdateResult>(`${binding(key)}/records/${encodeURIComponent(id)}`, {
        body: { values, expected_updated_at: expectedUpdatedAt },
      }),
    login: (email: string, password: string) =>
      api.post<LoginResult>(`${base}/auth/operator/login`, { body: { email, password } }),
    logout: () => api.post<Record<string, never>>(`${base}/auth/logout`, { body: {} }),
  };
}

export type SiteClient = ReturnType<typeof siteClient>;

/** Field issues of a `validation_failed` refusal (`details.errors[]`), empty for any other error. */
export function fieldIssues(error: unknown): FieldIssue[] {
  if (!(error instanceof ApiError) || error.errorCode !== "validation_failed") return [];
  const details = error.details;
  if (typeof details !== "object" || details === null) return [];
  const list = (details as { errors?: unknown }).errors;
  if (!Array.isArray(list)) return [];
  return list.filter(
    (item): item is FieldIssue =>
      typeof item === "object" && item !== null && typeof (item as FieldIssue).code === "string",
  );
}

/** The field key an issue points at: `field`, else the last segment of the JSON pointer. */
export function issueField(issue: FieldIssue): string | undefined {
  if (issue.field) return issue.field;
  if (!issue.path) return undefined;
  const parts = issue.path.split("/").filter(Boolean);
  const last = parts[parts.length - 1];
  return last ? last.replace(/~1/g, "/").replace(/~0/g, "~") : undefined;
}
