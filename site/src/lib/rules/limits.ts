/** Vendored from frontend/src/lib/forms/rules/limits.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * Limits shared with the Rust engine (`crates/sv-rules/src/limits.rs`). Changing a number here
 * requires the same change there.
 */

/** Maximum CEL source length in UTF-8 bytes (`cel_too_long`). */
export const MAX_SOURCE_BYTES = 1024;
/** Maximum lexical bracket depth over `(`, `[`, `{`, strings included (`cel_too_deep`). */
export const MAX_BRACKET_DEPTH = 24;
/** Maximum number of `?` characters, bounding conditional nesting (`cel_too_deep`). */
export const MAX_CONDITIONALS = 24;
/** Maximum number of source-level AST nodes (`cel_too_complex`). */
export const MAX_AST_NODES = 200;
/** Maximum static cost (`cel_too_complex`). */
export const MAX_STATIC_COST = 20_000;
/** Cost of one node inside an `all`/`exists` predicate. */
export const COMPREHENSION_COST_FACTOR = 256;
/** Maximum list and map length in bound variables (`cel_limit`). */
export const MAX_LIST_LEN = 256;
/** Maximum UTF-8 byte length of a bound string (`cel_limit`). */
export const MAX_STRING_BYTES = 64 * 1024;
/** Maximum nesting depth of a bound value (`cel_limit`). */
export const MAX_VALUE_DEPTH = 16;
/** Largest integer magnitude a bound variable may carry (`cel_limit`). */
export const MAX_SAFE_INTEGER = Number.MAX_SAFE_INTEGER;
/** Maximum regex pattern length in UTF-8 bytes. */
export const MAX_PATTERN_BYTES = 512;
/** Largest quantifier bound (`pattern_unsupported`). */
export const MAX_REPEAT = 1000;
/** Size budget of an expanded pattern (`pattern_unsupported`); see `regex-subset.ts`. */
export const MAX_REPEAT_UNITS = 4500;
/** Size units of a class, class escape or `.` (a literal is 1). */
export const CLASS_UNITS = 10;
/** Largest `min_length` / `max_length` bound. */
export const MAX_LENGTH_BOUND = 1_048_576;
/** Number of values an `enum` rule may list. */
export const MAX_ENUM_VALUES = 256;
/** 0001-01-01T00:00:00Z in Unix seconds. */
export const MIN_TIMESTAMP = -62_135_596_800n;
/** 9999-12-31T23:59:59Z in Unix seconds. */
export const MAX_TIMESTAMP = 253_402_300_799n;

const encoder = new TextEncoder();

/** UTF-8 byte length, the unit of the source, string and pattern limits. */
export function utf8Length(s: string): number {
  return encoder.encode(s).length;
}
