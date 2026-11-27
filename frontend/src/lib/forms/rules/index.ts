/**
 * Client mirror of the server rule engine (`crates/sv-rules`): built-in rules, the CEL subset and
 * the regex subset. The server stays authoritative; this module gives the designer and the record
 * editor the same verdicts. Conformance: `bun scripts/cel-conformance.ts <fixtures.json>`.
 */

export {
	RULE_KINDS,
	checkBuiltin,
	parseRule,
	ruleParams,
	type Bound,
	type BuiltinRule,
	type Operand,
	type ParseRuleResult,
	type PluginRuleRef,
	type RuleDefError,
	type RuleKind,
	type RuleSpec,
	type RuleSpecKind
} from './builtin.ts';
export {
	CelProgram,
	ENV_VARIABLES,
	compile,
	type CelEnv,
	type CelValue,
	type CompileResult,
	type EvalCode,
	type EvalResult
} from './cel.ts';
export type { CompileCode } from './cel-subset.ts';
export { compareCanonicalDecimal } from './decimal.ts';
export { checkPattern, compilePattern, type PatternCode } from './regex-subset.ts';
export { isEmail, isUrl } from './text.ts';
export * as limits from './limits.ts';
