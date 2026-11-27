/** Vendored from frontend/src/lib/forms/rules/index.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * Client mirror of the server rule engine (`crates/sv-rules`): built-in rules, the CEL subset and
 * the regex subset. The server stays authoritative; this module gives the designer and the record
 * editor the same verdicts. Conformance: `bun scripts/cel-conformance.ts <fixtures.json>`.
 */

export {
  type Bound,
  type BuiltinRule,
  checkBuiltin,
  type Operand,
  type ParseRuleResult,
  type PluginRuleRef,
  parseRule,
  RULE_KINDS,
  type RuleDefError,
  type RuleKind,
  type RuleSpec,
  type RuleSpecKind,
  ruleParams,
} from "./builtin";
export {
  type CelEnv,
  CelProgram,
  type CelValue,
  type CompileResult,
  compile,
  ENV_VARIABLES,
  type EvalCode,
  type EvalResult,
} from "./cel";
export type { CompileCode } from "./cel-subset";
export { compareCanonicalDecimal } from "./decimal";
export * as limits from "./limits";
export { checkPattern, compilePattern, type PatternCode } from "./regex-subset";
export { isEmail, isUrl } from "./text";
