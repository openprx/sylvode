/** Vendored from frontend/src/lib/forms/rules/cel.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * The CEL subset in the browser: compile (pre-checks, cel-js parse, subset walk) and evaluate with
 * cel-js. Mirror of `crates/sv-rules/src/expr/mod.rs`; read its module docs for the language,
 * timestamps (Unix seconds as `int`) and value binding. The server stays the only authority; this
 * module gives the designer preview the same verdicts.
 */

import { type ASTNode, Environment, type ParseResult } from "@marcbachmann/cel-js";
import { type CompileCode, hasBadHas, precheck, walk } from "./cel-subset";
import { compareCanonicalDecimal } from "./decimal";
import {
  MAX_LIST_LEN,
  MAX_SAFE_INTEGER,
  MAX_STRING_BYTES,
  MAX_TIMESTAMP,
  MAX_VALUE_DEPTH,
  MIN_TIMESTAMP,
  utf8Length,
} from "./limits";
import { parseIsoDate } from "./text";

export type CelEnv = "field_rule" | "default" | "layout_when";
export type EvalCode = "cel_error" | "cel_limit";

/** Variables per environment (contract C2 §5.2). */
export const ENV_VARIABLES: Readonly<Record<CelEnv, readonly string[]>> = {
  field_rule: ["value", "record", "now", "user"],
  default: ["now", "user"],
  layout_when: ["params", "data", "slot", "viewer"],
};

/** A CEL value as cel-js returns it: bigint for int, number for double, Map or object for map. */
export type CelValue = unknown;

export type EvalResult = { ok: true; value: CelValue } | { ok: false; code: EvalCode };

const SECONDS_PER_DAY = 86_400n;
const MAX_INT64 = 9_223_372_036_854_775_807n;
const MIN_INT64 = -9_223_372_036_854_775_808n;

class EvalFailure extends Error {
  constructor(readonly code: EvalCode) {
    super(code);
  }
}

function fail(): never {
  throw new Error("unsupported argument");
}

function utc(seconds: bigint): Date {
  if (seconds < MIN_TIMESTAMP || seconds > MAX_TIMESTAMP) fail();
  return new Date(Number(seconds) * 1000);
}

function isEmpty(v: unknown): boolean {
  if (v === null || v === undefined || v === "") return true;
  if (Array.isArray(v)) return v.length === 0;
  if (v instanceof Map) return v.size === 0;
  if (typeof v === "object" && v.constructor === Object) return Object.keys(v).length === 0;
  return false;
}

/**
 * Builds the evaluation environment for one CEL environment and one set of `matches` patterns.
 * cel-js keeps its own `size`, `int`, `double`, `string`, `contains`, `startsWith`, `endsWith` and
 * `matches`; the Rust side emulates them. `int == double` comparisons follow CEL's numeric
 * equality on both sides.
 */
function environment(env: CelEnv): Environment {
  const e = new Environment({
    unlistedVariablesAreDyn: false,
    homogeneousAggregateLiterals: false,
    enableOptionalTypes: false,
  });
  for (const name of ENV_VARIABLES[env]) e.registerVariable(name, "dyn");
  e.registerOperator("int == double", (a: bigint, b: number) => Number(a) === b);
  e.registerFunction("lower(string): string", (s: string) => s.toLowerCase());
  e.registerFunction("upper(string): string", (s: string) => s.toUpperCase());
  e.registerFunction("trim(string): string", (s: string) => s.trim());
  e.registerFunction("is_empty(dyn): bool", isEmpty);
  e.registerFunction("decimal_cmp(string, string): int", (a: string, b: string) => {
    const r = compareCanonicalDecimal(a, b);
    if (r === null) fail();
    return BigInt(r);
  });
  e.registerFunction("date(string): int", (s: string) => {
    const days = parseIsoDate(s);
    if (days === null) fail();
    return BigInt(days) * SECONDS_PER_DAY;
  });
  e.registerFunction("days_between(int, int): int", (a: bigint, b: bigint) => {
    const d = b - a;
    if (d > MAX_INT64 || d < MIN_INT64) fail();
    return d / SECONDS_PER_DAY;
  });
  const getter = (name: string, f: (d: Date) => number) =>
    e.registerFunction({
      name,
      receiverType: "int",
      returnType: "int",
      handler: (s: bigint) => BigInt(f(utc(s))),
      params: [],
    });
  getter("getFullYear", (d) => d.getUTCFullYear());
  getter("getMonth", (d) => d.getUTCMonth());
  getter("getDate", (d) => d.getUTCDate());
  getter("getDayOfWeek", (d) => d.getUTCDay());
  getter("getHours", (d) => d.getUTCHours());
  return e;
}

const environments = new Map<CelEnv, Environment>();

function environmentFor(env: CelEnv): Environment {
  let e = environments.get(env);
  if (!e) {
    e = environment(env);
    environments.set(env, e);
  }
  return e;
}

/** JSON to a CEL value, with the runtime limits of the Rust binding. */
export function bindValue(v: unknown, depth = 0): CelValue {
  if (depth > MAX_VALUE_DEPTH) throw new EvalFailure("cel_limit");
  if (v === null || typeof v === "boolean") return v;
  if (typeof v === "number") {
    if (Number.isInteger(v)) {
      if (Math.abs(v) > MAX_SAFE_INTEGER) throw new EvalFailure("cel_limit");
      return BigInt(v);
    }
    if (!Number.isFinite(v)) throw new EvalFailure("cel_limit");
    return v;
  }
  if (typeof v === "string") {
    if (utf8Length(v) > MAX_STRING_BYTES) throw new EvalFailure("cel_limit");
    return v;
  }
  if (Array.isArray(v)) {
    if (v.length > MAX_LIST_LEN) throw new EvalFailure("cel_limit");
    return v.map((item) => bindValue(item, depth + 1));
  }
  if (typeof v === "object") {
    const entries = Object.entries(v as Record<string, unknown>);
    if (entries.length > MAX_LIST_LEN) throw new EvalFailure("cel_limit");
    const m = new Map<string, CelValue>();
    for (const [k, val] of entries) {
      if (utf8Length(k) > MAX_STRING_BYTES) throw new EvalFailure("cel_limit");
      m.set(k, bindValue(val, depth + 1));
    }
    return m;
  }
  throw new EvalFailure("cel_limit");
}

/** A checked expression, ready to evaluate. */
export class CelProgram {
  constructor(
    readonly env: CelEnv,
    readonly source: string,
    /** Source-level node count. */
    readonly nodes: number,
    /** Static cost. */
    readonly cost: number,
    private readonly parsed: ParseResult,
  ) {}

  /** Evaluates with JSON variables; names outside the environment are ignored. */
  evaluate(vars: Readonly<Record<string, unknown>>): EvalResult {
    const ctx: Record<string, CelValue> = {};
    try {
      for (const name of ENV_VARIABLES[this.env]) {
        if (Object.hasOwn(vars, name)) ctx[name] = bindValue(vars[name]);
      }
    } catch (e) {
      if (e instanceof EvalFailure) return { ok: false, code: e.code };
      throw e;
    }
    try {
      return { ok: true, value: this.parsed(ctx) };
    } catch {
      return { ok: false, code: "cel_error" };
    }
  }

  /** Evaluates a boolean program; a non-boolean result is `cel_error`. */
  evalBool(
    vars: Readonly<Record<string, unknown>>,
  ): { ok: true; value: boolean } | { ok: false; code: EvalCode } {
    const r = this.evaluate(vars);
    if (!r.ok) return r;
    return typeof r.value === "boolean"
      ? { ok: true, value: r.value }
      : { ok: false, code: "cel_error" };
  }
}

export type CompileResult = { ok: true; program: CelProgram } | { ok: false; code: CompileCode };

/**
 * Compiles an expression for `env`: length and bracket checks, cel-js parse, the subset walk, the
 * identifier check, the node and cost limits and a shallow result-type check, in the order and
 * with the codes of the Rust `compile`.
 */
export function compile(src: string, env: CelEnv): CompileResult {
  const pre = precheck(src);
  if (pre) return { ok: false, code: pre };
  let parsed: ParseResult;
  try {
    parsed = environmentFor(env).parse(src);
  } catch {
    return { ok: false, code: "cel_syntax" };
  }
  const ast: ASTNode = parsed.ast;
  if (hasBadHas(ast)) return { ok: false, code: "cel_syntax" };
  const checked = walk(ast, ENV_VARIABLES[env], env === "field_rule");
  if (typeof checked === "string") return { ok: false, code: checked };
  return { ok: true, program: new CelProgram(env, src, checked.nodes, checked.cost, parsed) };
}
