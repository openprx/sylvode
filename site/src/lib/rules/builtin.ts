/** Vendored from frontend/src/lib/forms/rules/builtin.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * Rule objects of `rules.validate` and the built-in rule checks; mirror of
 * `crates/sv-rules/src/builtin.rs` (same kinds, parameters, error codes and JSON Pointer paths).
 */

import { type CelProgram, compile } from "./cel";
import { compareCanonicalDecimal, isCanonicalDecimal } from "./decimal";
import { MAX_ENUM_VALUES, MAX_LENGTH_BOUND } from "./limits";
import { compilePattern } from "./regex-subset";
import { isEmail, isUrl, parseIsoDate, parseRfc3339, scalarLength } from "./text";

export const RULE_KINDS = [
  "min",
  "max",
  "min_length",
  "max_length",
  "pattern",
  "enum",
  "email",
  "url",
  "cel",
  "plugin",
] as const;
export type RuleKind = (typeof RULE_KINDS)[number];

export type Bound =
  | { kind: "int"; value: bigint }
  | { kind: "decimal"; value: string }
  /** Days since the epoch. */
  | { kind: "date"; value: number }
  /** Nanoseconds since the epoch. */
  | { kind: "datetime"; value: bigint };

/** The value a rule is checked against, produced by the field type descriptor. */
export type Operand =
  | { kind: "int"; value: bigint }
  | { kind: "decimal"; value: string }
  /** `YYYY-MM-DD`. */
  | { kind: "date"; value: string }
  /** RFC 3339. */
  | { kind: "datetime"; value: string }
  | { kind: "str"; value: string }
  | { kind: "count"; value: number };

export type BuiltinRule =
  | { kind: "min" | "max"; bound: Bound; raw: unknown }
  | { kind: "min_length" | "max_length"; value: number }
  | { kind: "pattern"; source: string; caseInsensitive: boolean; regex: RegExp }
  | { kind: "enum"; values: string[] }
  | { kind: "email" }
  | { kind: "url"; schemes: string[] };

export interface PluginRuleRef {
  package: string;
  rule: string;
  deps: string[];
  config: Record<string, unknown>;
}

export type RuleSpecKind =
  | { type: "builtin"; rule: BuiltinRule }
  | { type: "cel"; program: CelProgram }
  | { type: "plugin"; plugin: PluginRuleRef };

export interface RuleSpec {
  spec: RuleSpecKind;
  code: string | null;
  message: Record<string, string> | null;
  runOnEmpty: boolean;
}

export interface RuleDefError {
  path: string;
  code: string;
}

export type ParseRuleResult = { ok: true; rule: RuleSpec } | { ok: false; error: RuleDefError };

const URL_SCHEMES = ["https", "http"];
const MAX_DEPS = 16;
const CODE = /^[a-z][a-z0-9_]{0,39}$/;

class DefFailure extends Error {
  constructor(
    readonly path: string,
    readonly code: string,
  ) {
    super(code);
  }
}

function pointer(key: string): string {
  return `/${key.replace(/~/g, "~0").replace(/\//g, "~1")}`;
}

function paramErr(key: string): DefFailure {
  return new DefFailure(pointer(key), "rule_param_invalid");
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** A JSON integer within ±(2^53 - 1); the Rust side accepts the same range. */
function jsonInt(v: unknown): number | null {
  return typeof v === "number" && Number.isSafeInteger(v) ? v : null;
}

function parseBound(v: unknown): Bound | null {
  if (typeof v === "number") {
    const i = jsonInt(v);
    return i === null ? null : { kind: "int", value: BigInt(i) };
  }
  if (typeof v !== "string") return null;
  if (isCanonicalDecimal(v)) return { kind: "decimal", value: v };
  if (v.length === 10) {
    const days = parseIsoDate(v);
    return days === null ? null : { kind: "date", value: days };
  }
  const ns = parseRfc3339(v);
  return ns === null ? null : { kind: "datetime", value: ns };
}

function lengthBound(v: unknown): number | null {
  const i = jsonInt(v);
  return i !== null && i >= 0 && i <= MAX_LENGTH_BOUND ? i : null;
}

function parseBuiltin(
  name: string,
  obj: Record<string, unknown>,
): [BuiltinRule, readonly string[]] {
  const value = obj.value;
  switch (name) {
    case "min":
    case "max": {
      if (!("value" in obj)) throw paramErr("value");
      const bound = parseBound(value);
      if (!bound) throw paramErr("value");
      return [{ kind: name, bound, raw: value }, ["value"]];
    }
    case "min_length":
    case "max_length": {
      const n = lengthBound(value);
      if (n === null) throw paramErr("value");
      return [{ kind: name, value: n }, ["value"]];
    }
    case "pattern": {
      if (typeof value !== "string") throw paramErr("value");
      let ci = false;
      if ("case_insensitive" in obj) {
        if (typeof obj.case_insensitive !== "boolean") throw paramErr("case_insensitive");
        ci = obj.case_insensitive;
      }
      const regex = compilePattern(value, ci);
      if (!(regex instanceof RegExp)) throw new DefFailure("/value", regex);
      return [
        { kind: "pattern", source: value, caseInsensitive: ci, regex },
        ["value", "case_insensitive"],
      ];
    }
    case "enum": {
      const values = obj.values;
      if (
        !Array.isArray(values) ||
        values.length === 0 ||
        values.length > MAX_ENUM_VALUES ||
        !values.every((v) => typeof v === "string")
      ) {
        throw paramErr("values");
      }
      return [{ kind: "enum", values: [...values] }, ["values"]];
    }
    case "email":
      return [{ kind: "email" }, []];
    case "url": {
      let schemes = ["https"];
      if ("schemes" in obj) {
        const s = obj.schemes;
        if (!Array.isArray(s) || s.length === 0) throw paramErr("schemes");
        schemes = [];
        for (const item of s) {
          if (typeof item !== "string" || !URL_SCHEMES.includes(item) || schemes.includes(item)) {
            throw paramErr("schemes");
          }
          schemes.push(item);
        }
      }
      return [{ kind: "url", schemes }, ["schemes"]];
    }
  }
  throw new DefFailure("/rule", "rule_unknown");
}

function parsePlugin(obj: Record<string, unknown>): PluginRuleRef {
  const plugin = obj.plugin;
  if (!isObject(plugin)) throw paramErr("plugin");
  const text = (key: string): string => {
    const v = plugin[key];
    if (typeof v !== "string" || v === "") {
      throw new DefFailure(`/plugin${pointer(key)}`, "rule_param_invalid");
    }
    return v;
  };
  const pkg = text("package");
  const rule = text("rule");
  if (Object.keys(plugin).some((k) => k !== "package" && k !== "rule")) throw paramErr("plugin");
  let deps: string[] = [];
  if ("deps" in obj) {
    const d = obj.deps;
    if (
      !Array.isArray(d) ||
      d.length > MAX_DEPS ||
      !d.every((x) => typeof x === "string" && x !== "")
    ) {
      throw paramErr("deps");
    }
    deps = [...d];
  }
  let config: Record<string, unknown> = {};
  if ("config" in obj) {
    if (!isObject(obj.config)) throw paramErr("config");
    config = obj.config;
  }
  return { package: pkg, rule, deps, config };
}

function parseRuleInner(v: unknown, allowed: readonly RuleKind[]): RuleSpec {
  if (!isObject(v)) throw new DefFailure("", "rule_param_invalid");
  const present = (["rule", "cel", "plugin"] as const).filter((k) => k in v);
  if (present.length === 0) throw new DefFailure("", "rule_unknown");
  if (present.length > 1) throw new DefFailure("", "rule_param_invalid");
  const selector = present[0];
  let kind: RuleKind;
  let spec: RuleSpecKind;
  let params: readonly string[];
  if (selector === "rule") {
    const name = v.rule;
    const k = RULE_KINDS.find((r) => r === name);
    if (typeof name !== "string" || !k || k === "cel" || k === "plugin") {
      throw new DefFailure("/rule", "rule_unknown");
    }
    if (!allowed.includes(k)) throw new DefFailure("/rule", "rule_not_allowed_for_type");
    const [rule, p] = parseBuiltin(name, v);
    kind = k;
    spec = { type: "builtin", rule };
    params = p;
  } else if (selector === "cel") {
    if (!allowed.includes("cel")) throw new DefFailure("/cel", "rule_not_allowed_for_type");
    if (typeof v.cel !== "string") throw paramErr("cel");
    const r = compile(v.cel, "field_rule");
    if (!r.ok) throw new DefFailure("/cel", r.code);
    kind = "cel";
    spec = { type: "cel", program: r.program };
    params = [];
  } else {
    if (!allowed.includes("plugin")) throw new DefFailure("/plugin", "rule_not_allowed_for_type");
    kind = "plugin";
    spec = { type: "plugin", plugin: parsePlugin(v) };
    params = ["deps", "config"];
  }
  const runsOnEmpty = kind === "cel" || kind === "plugin";
  for (const key of Object.keys(v).sort()) {
    const known =
      key === selector ||
      params.includes(key) ||
      key === "code" ||
      key === "message" ||
      (key === "run_on_empty" && runsOnEmpty);
    if (!known) throw paramErr(key);
  }
  let code: string | null = null;
  if ("code" in v) {
    if (typeof v.code !== "string" || !CODE.test(v.code)) throw paramErr("code");
    code = v.code;
  }
  let message: Record<string, string> | null = null;
  if ("message" in v) {
    const m = v.message;
    if (!isObject(m) || !Object.values(m).every((x) => typeof x === "string")) {
      throw paramErr("message");
    }
    message = m as Record<string, string>;
  }
  let runOnEmpty = false;
  if ("run_on_empty" in v) {
    if (typeof v.run_on_empty !== "boolean") throw paramErr("run_on_empty");
    runOnEmpty = v.run_on_empty;
  }
  return { spec, code, message, runOnEmpty };
}

/** Parses one `rules.validate` entry; `allowed` is the field type's rule list. */
export function parseRule(v: unknown, allowed: readonly RuleKind[] = RULE_KINDS): ParseRuleResult {
  try {
    return { ok: true, rule: parseRuleInner(v, allowed) };
  } catch (e) {
    if (e instanceof DefFailure) return { ok: false, error: { path: e.path, code: e.code } };
    throw e;
  }
}

function compareBound(op: Operand, bound: Bound): -1 | 0 | 1 | null {
  const sign = (d: bigint | number): -1 | 0 | 1 => (d < 0 ? -1 : d > 0 ? 1 : 0);
  if (op.kind === "int" && bound.kind === "int") return sign(op.value - bound.value);
  if (op.kind === "int" && bound.kind === "decimal") {
    return compareCanonicalDecimal(op.value.toString(), bound.value);
  }
  if (op.kind === "decimal" && (bound.kind === "decimal" || bound.kind === "int")) {
    return compareCanonicalDecimal(op.value, bound.value.toString());
  }
  if (op.kind === "date" && bound.kind === "date") {
    const days = parseIsoDate(op.value);
    return days === null ? null : sign(days - bound.value);
  }
  if (op.kind === "datetime" && bound.kind === "datetime") {
    const ns = parseRfc3339(op.value);
    return ns === null ? null : sign(ns - bound.value);
  }
  return null;
}

function text(op: Operand): string | null {
  return op.kind === "str" ? op.value : null;
}

function length(op: Operand): number | null {
  if (op.kind === "str") return scalarLength(op.value);
  if (op.kind === "count") return op.value;
  return null;
}

/**
 * Checks a non-empty operand: `null` on pass, else the rule's error code, or `rule_invalid` when
 * the operand kind does not fit the rule. The caller applies a `code` override.
 */
export function checkBuiltin(rule: BuiltinRule, op: Operand): string | null {
  let pass: boolean | null;
  switch (rule.kind) {
    case "min":
    case "max": {
      const c = compareBound(op, rule.bound);
      pass = c === null ? null : rule.kind === "min" ? c !== -1 : c !== 1;
      break;
    }
    case "min_length":
    case "max_length": {
      const n = length(op);
      pass = n === null ? null : rule.kind === "min_length" ? n >= rule.value : n <= rule.value;
      break;
    }
    case "pattern": {
      const s = text(op);
      pass = s === null ? null : rule.regex.test(s);
      break;
    }
    case "enum": {
      const s = text(op);
      pass = s === null ? null : rule.values.includes(s);
      break;
    }
    case "email": {
      const s = text(op);
      pass = s === null ? null : isEmail(s);
      break;
    }
    case "url": {
      const s = text(op);
      pass = s === null ? null : isUrl(s, rule.schemes);
      break;
    }
  }
  if (pass === null) return "rule_invalid";
  return pass ? null : rule.kind;
}

/** Error `params` for a failure: schema constants only. */
export function ruleParams(rule: BuiltinRule): Record<string, unknown> | null {
  switch (rule.kind) {
    case "min":
    case "max":
      return { bound: rule.raw };
    case "min_length":
    case "max_length":
      return { bound: rule.value };
    default:
      return null;
  }
}
