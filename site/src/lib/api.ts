/**
 * Typed fetch helper for the site API. The site API answers with real HTTP status codes and the
 * console envelope `{code, message, data, error_code?, details?}` (contract C3 section 6.6, C3-1):
 * `request` returns `data` of a 2xx answer and throws `ApiError` otherwise.
 */

const DEFAULT_BASE = "/api/site/v1";

export const apiBase: string = (import.meta.env.VITE_SV_SITE_API ?? DEFAULT_BASE).replace(
  /\/+$/,
  "",
);

export interface Envelope<T> {
  code: number;
  message?: string;
  data: T;
  error_code?: string;
  details?: unknown;
}

export class ApiError extends Error {
  readonly status: number;
  readonly body: unknown;
  /** Typed refusal code of the envelope (`validation_failed`, `operator_required`, ...). */
  readonly errorCode: string | undefined;
  readonly details: unknown;

  constructor(status: number, body: unknown) {
    const envelope = isEnvelope(body) ? body : undefined;
    super(envelope?.message || `request failed with status ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
    this.errorCode = envelope?.error_code;
    this.details = envelope?.details;
  }
}

function isEnvelope(value: unknown): value is Partial<Envelope<unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export interface RequestOptions {
  query?: Record<string, string | number | boolean | undefined>;
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
}

function buildUrl(path: string, query: RequestOptions["query"]): string {
  const url = `${apiBase}${path.startsWith("/") ? path : `/${path}`}`;
  if (!query) return url;
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) params.set(key, String(value));
  }
  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

export async function request<T>(
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
  path: string,
  options: RequestOptions = {},
): Promise<T> {
  const headers: Record<string, string> = { accept: "application/json", ...options.headers };
  // The site API needs the visitor/operator cookies (possibly cross-origin) and, on every write,
  // the marker header `X-SV-Site` (contract section 6.4).
  if (method !== "GET") headers["x-sv-site"] = "1";
  const init: RequestInit = { method, headers, credentials: "include" };
  if (options.signal) init.signal = options.signal;
  if (options.body !== undefined) {
    headers["content-type"] = "application/json";
    init.body = JSON.stringify(options.body);
  }
  const response = await fetch(buildUrl(path, options.query), init);
  const text = await response.text();
  let parsed: unknown;
  try {
    parsed = text ? JSON.parse(text) : undefined;
  } catch {
    parsed = undefined;
  }
  if (!response.ok) throw new ApiError(response.status, parsed);
  if (isEnvelope(parsed) && "data" in parsed) return parsed.data as T;
  return parsed as T;
}

export const api = {
  get: <T>(path: string, options?: RequestOptions) => request<T>("GET", path, options),
  post: <T>(path: string, options?: RequestOptions) => request<T>("POST", path, options),
  put: <T>(path: string, options?: RequestOptions) => request<T>("PUT", path, options),
  patch: <T>(path: string, options?: RequestOptions) => request<T>("PATCH", path, options),
  delete: <T>(path: string, options?: RequestOptions) => request<T>("DELETE", path, options),
};
