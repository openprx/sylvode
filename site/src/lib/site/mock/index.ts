import { apiBase } from "../../api";
import type { Manifest, SiteRecord } from "../types";
import {
  BINDING,
  FIELDS_RESPONSE,
  MOCK_OPERATOR,
  MOCK_SLUG,
  PAGES,
  SITE,
  seedRecords,
} from "./fixtures";

/**
 * Fixture-backed `fetch` for the site API, in the shapes of contract C3 section 6.6 (real HTTP
 * statuses, the console envelope). Installed only by `VITE_SV_SITE_MOCK=1`; state lives in memory,
 * so a reload starts from the seed again.
 */

interface State {
  records: (SiteRecord & { owner: string | null })[];
  visitor: string | null;
  operator: boolean;
  nextId: number;
}

const STORE_KEY = "sv-site-mock-state";

function initialState(): State {
  try {
    const saved = sessionStorage.getItem(STORE_KEY);
    if (saved) return JSON.parse(saved) as State;
  } catch {
    // fall through to the seed
  }
  return {
    records: seedRecords().map((record, index) => ({
      ...record,
      owner: index === 0 ? "v1" : null,
    })),
    visitor: "v1",
    operator: false,
    nextId: 3,
  };
}

const state: State = initialState();

/** Keeps the in-memory backend across reloads of the tab (a reload would otherwise reset it). */
function persist(): void {
  try {
    sessionStorage.setItem(STORE_KEY, JSON.stringify(state));
  } catch {
    // storage unavailable: state stays in memory only
  }
}

/** Path part of the configured API base (it may be an absolute URL). */
function basePath(): string {
  return new URL(apiBase, window.location.origin).pathname.replace(/\/+$/, "");
}

function reply(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const ok = (data: unknown, status = 200) => reply(status, { code: 0, message: "ok", data });
const fail = (status: number, errorCode: string, message: string, details?: unknown) =>
  reply(status, {
    code: status,
    message,
    data: null,
    error_code: errorCode,
    ...(details ? { details } : {}),
  });

function manifest(): Manifest {
  return {
    site: SITE,
    operator: state.operator
      ? {
          user_id: MOCK_OPERATOR.user_id,
          display_name: MOCK_OPERATOR.display_name,
          role: MOCK_OPERATOR.role,
        }
      : null,
    pages: PAGES.filter((p) => state.operator || p.page.audience === "visitor").map((p) => p.page),
    bindings: [state.operator ? BINDING : { ...BINDING, operator: null }],
  };
}

function matchRoute(pattern: string, path: string): Record<string, string> | null {
  const a = pattern.split("/").filter(Boolean);
  const b = path.split("/").filter(Boolean);
  if (a.length !== b.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < a.length; i++) {
    const part = a[i] ?? "";
    const seg = b[i] ?? "";
    if (part.startsWith(":")) params[part.slice(1)] = seg;
    else if (part !== seg) return null;
  }
  return params;
}

function visible(record: SiteRecord & { owner: string | null }, fields: string[]) {
  const values = Object.fromEntries(
    Object.entries(record.values).filter(([key]) => fields.includes(key)),
  );
  return {
    id: record.id,
    title: record.title ?? null,
    values,
    created_at: record.created_at,
    updated_at: record.updated_at,
  };
}

function readable() {
  if (state.operator) {
    return {
      fields: BINDING.operator?.read_fields ?? [],
      rows: state.records,
    };
  }
  return {
    fields: BINDING.visitor.read_fields,
    rows: state.records.filter((r) => state.visitor !== null && r.owner === state.visitor),
  };
}

async function handle(method: string, url: URL, bodyText: string): Promise<Response> {
  const prefix = `${basePath()}/${MOCK_SLUG}`;
  const path = url.pathname.startsWith(prefix) ? url.pathname.slice(prefix.length) : null;
  if (path === null) return fail(404, "not_found", "unknown site");
  const body = bodyText ? (JSON.parse(bodyText) as Record<string, unknown>) : {};

  if (method === "GET" && path === "/manifest") return ok(manifest());

  if (method === "GET" && path === "/pages") {
    const route = url.searchParams.get("route") ?? "/";
    for (const entry of PAGES) {
      const params = matchRoute(entry.page.route, route);
      if (!params) continue;
      if (entry.page.audience === "operator" && !state.operator) {
        return fail(401, "operator_required", "operator session required");
      }
      return ok({ route, audience: entry.page.audience, params, layout: entry.layout });
    }
    return fail(404, "not_found", "no such page");
  }

  const binding = /^\/bindings\/([a-z_]+)(\/.*)?$/.exec(path);
  if (binding) {
    if (binding[1] !== BINDING.key) return fail(404, "not_found", "no such binding");
    const rest = binding[2] ?? "";
    if (method === "GET" && rest === "/fields") {
      const keep = new Set([
        ...BINDING.visitor.read_fields,
        ...BINDING.visitor.submit_fields,
        ...(state.operator ? [...(BINDING.operator?.read_fields ?? [])] : []),
      ]);
      return ok({ fields: FIELDS_RESPONSE.fields.filter((f) => keep.has(f.key)) });
    }
    if (method === "GET" && rest === "/records") {
      const { fields, rows } = readable();
      let items = rows;
      for (const [name, value] of url.searchParams) {
        const m = /^filter\[(.+)\]$/.exec(name);
        if (!m) continue;
        const field = m[1] ?? "";
        if (field !== "status") return fail(400, "field_not_filterable", "field not filterable");
        items = items.filter((r) => String(r.values[field]) === value);
      }
      const sort = url.searchParams.get("sort");
      if (sort) {
        const desc = sort.startsWith("-");
        const key = desc ? sort.slice(1) : sort;
        if (!fields.includes(key)) return fail(400, "field_not_filterable", "field not sortable");
        items = [...items].sort(
          (x, y) =>
            String(x.values[key] ?? "").localeCompare(String(y.values[key] ?? "")) *
            (desc ? -1 : 1),
        );
      }
      const perPage = Math.min(Number(url.searchParams.get("per_page") ?? 20), 100);
      const page = Math.max(Number(url.searchParams.get("page") ?? 1), 1);
      const slice = items.slice((page - 1) * perPage, page * perPage);
      return ok({
        items: slice.map((r) => visible(r, fields)),
        total: items.length,
        page,
        per_page: perPage,
        total_pages: Math.max(Math.ceil(items.length / perPage), 1),
      });
    }
    const one = /^\/records\/([^/]+)$/.exec(rest);
    if (method === "GET" && one) {
      const { fields, rows } = readable();
      const record = rows.find((r) => r.id === one[1]);
      return record ? ok(visible(record, fields)) : fail(404, "not_found", "not found");
    }
    if (method === "POST" && rest === "/records") {
      const values = (body.values ?? {}) as Record<string, unknown>;
      const allowed = BINDING.visitor.submit_fields;
      const stray = Object.keys(values).find((k) => !allowed.includes(k));
      if (stray) return fail(422, "field_not_allowed", `field ${stray} not allowed`);
      const issues: { field: string; code: string; message: string }[] = [];
      const title = typeof values.title === "string" ? values.title.trim() : "";
      if (!title) issues.push({ field: "title", code: "required", message: "Title is required" });
      else if (title.length > 80)
        issues.push({ field: "title", code: "max_length", message: "At most 80 characters" });
      if (issues.length > 0) {
        return fail(422, "validation_failed", "validation failed", { errors: issues });
      }
      state.visitor ??= "v1";
      const now = new Date().toISOString();
      const id = `00000000-0000-4000-8000-${String(state.nextId++).padStart(12, "0")}`;
      state.records.unshift({
        id,
        title,
        values: { ...values, title, status: "open", created_at: now },
        created_at: now,
        updated_at: now,
        owner: state.operator ? null : state.visitor,
      });
      return ok({ id, values }, 201);
    }
    if (method === "PATCH" && one) {
      if (!state.operator) return fail(401, "operator_required", "operator session required");
      const record = state.records.find((r) => r.id === one[1]);
      if (!record) return fail(404, "not_found", "not found");
      if (body.expected_updated_at !== record.updated_at) {
        return fail(409, "conflict", "record changed");
      }
      const values = (body.values ?? {}) as Record<string, unknown>;
      const allowed = BINDING.operator?.update_fields ?? [];
      const stray = Object.keys(values).find((k) => !allowed.includes(k));
      if (stray) return fail(422, "field_not_allowed", `field ${stray} not allowed`);
      Object.assign(record.values, values);
      record.updated_at = new Date().toISOString();
      return ok({ id: record.id, values: record.values, updated_at: record.updated_at });
    }
    return fail(404, "not_found", "no such route");
  }

  if (method === "POST" && path === "/auth/operator/login") {
    if (body.email === MOCK_OPERATOR.email && body.password === MOCK_OPERATOR.password) {
      state.operator = true;
      return ok({
        operator: {
          user_id: MOCK_OPERATOR.user_id,
          display_name: MOCK_OPERATOR.display_name,
          role: MOCK_OPERATOR.role,
        },
      });
    }
    return fail(401, "login_failed", "login failed");
  }
  if (method === "POST" && path === "/auth/logout") {
    state.operator = false;
    return ok({});
  }
  return fail(404, "not_found", "no such route");
}

/** Replaces `window.fetch` for site API URLs; everything else passes through. */
export function installMock(): void {
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const request = new Request(input, init);
    const url = new URL(request.url, window.location.origin);
    if (!url.pathname.startsWith(`${basePath()}/`)) return real(input, init);
    const response = await handle(request.method, url, await request.text());
    persist();
    return response;
  };
  console.info(`[sv-site] mock mode: site "${MOCK_SLUG}", operator ${MOCK_OPERATOR.email}`);
}
