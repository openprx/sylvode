import { type BuiltinRule, checkBuiltin, type Operand, parseRule } from "../rules";
import type { SiteField } from "../site/types";
import { inputKind } from "./types";

/** `YYYY-MM-DD` of a `datetime-local` or RFC 3339 value is not derived here; the server decides. */
function operand(field: SiteField, value: unknown): Operand | null {
  const kind = inputKind(field.type);
  if (kind === "multi_select")
    return Array.isArray(value) ? { kind: "count", value: value.length } : null;
  if (kind === "number") {
    const n = typeof value === "number" ? value : Number(value);
    if (!Number.isFinite(n)) return null;
    if (field.type === "integer" || field.type === "rating" || field.type === "progress") {
      return Number.isInteger(n) ? { kind: "int", value: BigInt(n) } : null;
    }
    return { kind: "decimal", value: String(n) };
  }
  if (kind === "date") return typeof value === "string" ? { kind: "date", value } : null;
  if (kind === "text" || kind === "textarea" || kind === "single_select") {
    return typeof value === "string" ? { kind: "str", value } : null;
  }
  return null;
}

export function isEmptyValue(value: unknown): boolean {
  return (
    value === undefined ||
    value === null ||
    value === "" ||
    (Array.isArray(value) && value.length === 0)
  );
}

/**
 * Client-side hints only (required and the built-in rules of `rules_client`); the C2 error report of
 * the server is the verdict. Returns an error code or null. Rules the mirror cannot evaluate here
 * (CEL, plugin) are skipped.
 */
export function validateField(field: SiteField, value: unknown): string | null {
  if (isEmptyValue(value)) return field.required ? "required" : null;
  const op = operand(field, value);
  if (!op) return null;
  for (const raw of field.rules_client ?? []) {
    const parsed = parseRule(raw);
    if (!parsed.ok || parsed.rule.spec.type !== "builtin") continue;
    const rule: BuiltinRule = parsed.rule.spec.rule;
    const verdict = checkBuiltin(rule, op);
    if (verdict !== null) return parsed.rule.code ?? verdict;
  }
  return null;
}
