import { type Accessor, createSignal } from "solid-js";
import { ApiError } from "../api";
import type { SiteClient } from "../site/client";
import { fieldIssues } from "../site/client";
import type { FieldIssue, Manifest, SiteField, SiteRecord } from "../site/types";
import { displayValue, resolveDeep, resolveText, resolveValue } from "./refs";
import type { ActionSpec, DataSourceSpec, FilterSpec, LayoutSpec, RefScope, Viewer } from "./types";

export type ActionStatus = "idle" | "running" | "ok" | "error";

export interface ActionState {
  status: ActionStatus;
  output?: unknown;
  /** Typed code of the refusal (`validation_failed`, `conflict`, `request_failed`, ...). */
  error?: string;
  issues?: FieldIssue[];
}

/** What the host supplies to a rendered layout. */
export interface RuntimeEnv {
  client: SiteClient;
  manifest: Manifest;
  viewer: Viewer;
  /** Path of the router (a manifest route). */
  navigate: (path: string) => void;
  confirm: (message: string) => Promise<boolean>;
  notify: (message: string, tone: "success" | "danger") => void;
  /** Called after a write succeeded, so surrounding views can refetch. */
  onWritten?: () => void;
}

export type SourceResult = { ok: true; value: unknown } | { ok: false; code: string };

const BINDING = /^binding:([a-z][a-z0-9_]{0,39})$/;
const PAGE_LIMIT = 100;

function pickValues(values: Record<string, unknown>, keys: string[] | undefined) {
  if (!keys) return values;
  return Object.fromEntries(Object.entries(values).filter(([key]) => keys.includes(key)));
}

function recordRow(record: SiteRecord, keys: string[] | undefined) {
  return {
    id: record.id,
    title: record.title ?? null,
    values: pickValues(record.values, keys),
    created_at: record.created_at,
    updated_at: record.updated_at,
  };
}

function matchesFilter(record: SiteRecord, filter: FilterSpec, value: unknown): boolean {
  const a = displayValue(record.values[filter.field]);
  const b = displayValue(value);
  switch (filter.op) {
    case "eq":
      return a === b;
    case "neq":
      return a !== b;
    case "contains":
      return a.toLowerCase().includes(b.toLowerCase());
    case "not_empty":
      return a !== "";
    case "gt":
      return Number(a) > Number(b);
    case "gte":
      return Number(a) >= Number(b);
    case "lt":
      return Number(a) < Number(b);
    case "lte":
      return Number(a) <= Number(b);
    default:
      return true;
  }
}

function errorCode(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.errorCode) return error.errorCode;
    if (error.status === 409 || error.status === 412) return "conflict";
    return `http_${error.status}`;
  }
  return "request_failed";
}

/**
 * State of one rendered layout on the site: data sources (read through the binding endpoints),
 * action status and the action runner. Counterpart of the console `LayoutRuntime`; the viewer is
 * the visitor or the operator, so every read is already filtered by the server.
 */
export class LayoutRuntime {
  readonly data: Accessor<Record<string, unknown>>;
  readonly errors: Accessor<Record<string, string>>;
  readonly loading: Accessor<boolean>;
  readonly actions: Accessor<Record<string, ActionState>>;

  readonly spec: LayoutSpec;
  readonly #params: Record<string, unknown>;
  readonly #env: RuntimeEnv;
  readonly #setData: (value: Record<string, unknown>) => void;
  readonly #setErrors: (value: Record<string, string>) => void;
  readonly #setLoading: (value: boolean) => void;
  readonly #setActions: (value: Record<string, ActionState>) => void;
  readonly #fields = new Map<string, Promise<SiteField[]>>();
  #actionsNow: Record<string, ActionState> = {};
  #dataNow: Record<string, unknown> = {};
  #generation = 0;

  constructor(spec: LayoutSpec, params: Record<string, unknown>, env: RuntimeEnv) {
    this.spec = spec;
    this.#params = params;
    this.#env = env;
    const [data, setData] = createSignal<Record<string, unknown>>({});
    const [errors, setErrors] = createSignal<Record<string, string>>({});
    const [loading, setLoading] = createSignal(true);
    const [actions, setActions] = createSignal<Record<string, ActionState>>({});
    this.data = data;
    this.errors = errors;
    this.loading = loading;
    this.actions = actions;
    this.#setData = (value) => setData(() => value);
    this.#setErrors = (value) => setErrors(() => value);
    this.#setLoading = (value) => setLoading(() => value);
    this.#setActions = (value) => setActions(() => value);
  }

  get viewer(): Viewer {
    return this.#env.viewer;
  }

  get locale(): string {
    return this.#env.viewer.locale;
  }

  /** The reference scope; `$slot` is empty on a site. */
  scope(): RefScope {
    return {
      params: this.#params,
      data: this.data(),
      slot: {},
      viewer: { ...this.#env.viewer },
      actions: this.actions(),
    };
  }

  #bindingKey(raw: unknown, scope: RefScope): string | null {
    const value = resolveValue(raw, scope);
    const match = typeof value === "string" ? BINDING.exec(value) : null;
    const key = match?.[1];
    if (!key || !this.#env.manifest.bindings.some((binding) => binding.key === key)) return null;
    return key;
  }

  #bindingFields(key: string): Promise<SiteField[]> {
    let cached = this.#fields.get(key);
    if (!cached) {
      cached = this.#env.client.fields(key).then((result) => result.fields);
      this.#fields.set(key, cached);
      cached.catch(() => this.#fields.delete(key));
    }
    return cached;
  }

  /** Keys a form for `mode` may edit: visitor submit fields, or the operator's update fields. */
  editableFields(key: string, mode: "create" | "update"): string[] {
    const binding = this.#env.manifest.bindings.find((item) => item.key === key);
    if (!binding) return [];
    if (mode === "update") return binding.operator?.update_fields ?? [];
    return binding.visitor.submit_fields;
  }

  async #loadSource(spec: DataSourceSpec, scope: RefScope): Promise<SourceResult> {
    const { client } = this.#env;
    switch (spec.source) {
      case "record": {
        const key = this.#bindingKey(spec.form, scope);
        if (!key) return { ok: false, code: "binding_missing" };
        const recordId = resolveValue(spec.record, scope);
        if (typeof recordId !== "string" || !recordId) return { ok: false, code: "record_missing" };
        const record = await client.getRecord(key, recordId);
        const all = await this.#bindingFields(key);
        const fields = all
          .filter((field) => (spec.fields ? spec.fields.includes(field.key) : true))
          .filter((field) => field.key in record.values)
          .map((field) => ({ key: field.key, label: field.label, type: field.type }));
        return { ok: true, value: { ...recordRow(record, spec.fields), fields } };
      }
      case "query": {
        const key = this.#bindingKey(spec.form, scope);
        if (!key) return { ok: false, code: "binding_missing" };
        const filters = (spec.filter ?? []).map((filter) => ({
          filter,
          value: resolveDeep(filter.value, scope),
        }));
        // The endpoint takes `filter[field]=value` (equality); other operators narrow the page here.
        const server: Record<string, string> = {};
        const client_side: typeof filters = [];
        for (const item of filters) {
          if (item.filter.op === "eq" && !(item.filter.field in server)) {
            server[item.filter.field] = displayValue(item.value);
          } else {
            client_side.push(item);
          }
        }
        const sort = spec.sort?.[0];
        const result = await client.listRecords(key, {
          per_page: Math.min(spec.page_size ?? 20, PAGE_LIMIT),
          ...(sort ? { sort: sort.dir === "desc" ? `-${sort.field}` : sort.field } : {}),
          filter: server,
        });
        const items = result.items.filter((record) =>
          client_side.every(({ filter, value }) => matchesFilter(record, filter, value)),
        );
        // Field labels give a `table` node its column headers; a failure here only costs the labels.
        const labels = await this.#bindingFields(key)
          .then((all) =>
            all
              .filter((field) => (spec.fields ? spec.fields.includes(field.key) : true))
              .map((field) => ({ key: field.key, label: field.label, type: field.type })),
          )
          .catch(() => []);
        return {
          ok: true,
          value: {
            items: items.map((record) => recordRow(record, spec.fields)),
            total: client_side.length ? items.length : result.total,
            fields: labels,
          },
        };
      }
      case "form": {
        const key = this.#bindingKey(spec.form, scope);
        if (!key) return { ok: false, code: "binding_missing" };
        const all = await this.#bindingFields(key);
        const fields = all
          .filter((field) => (spec.fields ? spec.fields.includes(field.key) : true))
          .map((field) => ({
            key: field.key,
            label: field.label,
            type: field.type,
            required: Boolean(field.required),
            read_only: false,
            ...(field.options?.length ? { options: field.options } : {}),
            ...(field.rules_client ? { rules_client: field.rules_client } : {}),
          }));
        return { ok: true, value: { form_id: key, fields } };
      }
      default:
        return { ok: false, code: "source_not_available" };
    }
  }

  async load(): Promise<void> {
    const generation = ++this.#generation;
    const sources = Object.entries(this.spec.data ?? {});
    const base: RefScope = { ...this.scope(), data: {}, actions: {} };
    const results = await Promise.all(
      sources.map(async ([name, source]) => {
        try {
          return [name, await this.#loadSource(source, base)] as const;
        } catch (error) {
          return [name, { ok: false, code: errorCode(error) } as const] as const;
        }
      }),
    );
    if (generation !== this.#generation) return;
    const data: Record<string, unknown> = {};
    const errors: Record<string, string> = {};
    for (const [name, result] of results) {
      if (result.ok) data[name] = result.value;
      else errors[name] = result.code;
    }
    this.#dataNow = data;
    this.#setData(data);
    this.#setErrors(errors);
    this.#setLoading(false);
  }

  /** Loads every source again after a failure, showing the loading state meanwhile. */
  retry(): Promise<void> {
    this.#setLoading(true);
    return this.load();
  }

  /** Why an action cannot run now (disabled tooltip), or null when it can. */
  unavailable(name: string | undefined): string | null {
    if (!name) return null;
    const action = this.spec.actions?.[name];
    if (!action) return "action_unknown";
    if (action.kind === "invoke") return "not_available_on_site";
    return null;
  }

  #set(name: string, state: ActionState): void {
    this.#actionsNow = { ...this.#actionsNow, [name]: state };
    this.#setActions(this.#actionsNow);
  }

  /** Runs a named action; `values` are the draft values of a `form` node for `submit`. */
  async run(name: string, values?: Record<string, unknown>): Promise<boolean> {
    const action = this.spec.actions?.[name];
    if (!action || this.unavailable(name)) return false;
    const scope = this.scope();
    if (action.kind === "submit" && action.confirm) {
      const message = resolveText(action.confirm, scope, this.locale);
      if (message && !(await this.#env.confirm(message))) return false;
    }
    this.#set(name, { status: "running" });
    try {
      const outcome =
        action.kind === "navigate"
          ? this.#navigate(action)
          : action.kind === "submit"
            ? await this.#submit(action, values ?? {}, scope)
            : { status: "error" as const, error: "not_available_on_site" };
      this.#set(name, outcome);
      if (outcome.status === "ok" && action.kind === "submit") {
        this.#env.onWritten?.();
        await this.load();
      }
      return outcome.status === "ok";
    } catch (error) {
      this.#set(name, {
        status: "error",
        error: errorCode(error),
        issues: fieldIssues(error),
      });
      return false;
    }
  }

  #navigate(action: Extract<ActionSpec, { kind: "navigate" }>): ActionState {
    const route = (action.to as { route?: unknown }).route;
    const target = resolveValue(route, this.scope());
    if (typeof target !== "string" || !target.startsWith("/") || target.startsWith("//")) {
      return { status: "error", error: "action_not_allowed" };
    }
    this.#env.navigate(target);
    return { status: "ok" };
  }

  async #currentUpdatedAt(key: string, recordId: string): Promise<string> {
    for (const value of Object.values(this.#dataNow)) {
      const row = value as { id?: unknown; updated_at?: unknown } | undefined;
      if (row?.id === recordId && typeof row.updated_at === "string") return row.updated_at;
    }
    return (await this.#env.client.getRecord(key, recordId)).updated_at;
  }

  async #submit(
    action: Extract<ActionSpec, { kind: "submit" }>,
    values: Record<string, unknown>,
    scope: RefScope,
  ): Promise<ActionState> {
    const source = this.spec.data?.[action.data];
    if (source?.source !== "form") return { status: "error", error: "action_unknown" };
    const key = this.#bindingKey(source.form, scope);
    if (!key) return { status: "error", error: "binding_missing" };
    const allowed = new Set(this.editableFields(key, action.mode));
    const body = Object.fromEntries(Object.entries(values).filter(([field]) => allowed.has(field)));
    try {
      if (action.mode === "update") {
        const recordId = resolveValue(action.record, scope);
        if (typeof recordId !== "string" || !recordId) {
          return { status: "error", error: "record_missing" };
        }
        const updatedAt = await this.#currentUpdatedAt(key, recordId);
        const result = await this.#env.client.update(key, recordId, body, updatedAt);
        this.#env.notify("saved", "success");
        return { status: "ok", output: result };
      }
      const result = await this.#env.client.submit(key, body);
      this.#env.notify("submitted", "success");
      return { status: "ok", output: result };
    } catch (error) {
      return { status: "error", error: errorCode(error), issues: fieldIssues(error) };
    }
  }
}
