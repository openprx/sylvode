//! Limits shared by the Rust engine and the TypeScript mirror (`frontend/src/lib/forms/rules/limits.ts`).
//! Changing a number here requires the same change there.

/// Maximum CEL source length in UTF-8 bytes (`cel_too_long`).
pub const MAX_SOURCE_BYTES: usize = 1024;
/// Maximum lexical bracket depth, counted over `(`, `[`, `{` before the parser runs (`cel_too_deep`).
/// Brackets inside string literals are counted too, so the check needs no tokenizer.
pub const MAX_BRACKET_DEPTH: usize = 24;
/// Maximum number of conditional operators (`?`) in a source; the parser recurses once per
/// conditional, so this bounds parser recursion together with the bracket depth (`cel_too_deep`).
pub const MAX_CONDITIONALS: usize = 24;
/// Maximum number of source-level AST nodes (`cel_too_complex`).
pub const MAX_AST_NODES: usize = 200;
/// Maximum static cost; a node costs 1, or `COMPREHENSION_COST_FACTOR` inside the predicate of an
/// `all`/`exists` macro (`cel_too_complex`).
pub const MAX_STATIC_COST: u64 = 20_000;
/// Cost of one node inside a macro predicate.
pub const COMPREHENSION_COST_FACTOR: u64 = 256;
/// Maximum list (and map) length in bound variables; macro ranges are bound variables or literals,
/// so this is also the largest list `all`/`exists` can see (`cel_limit`).
pub const MAX_LIST_LEN: usize = 256;
/// Maximum UTF-8 byte length of a string in bound variables (`cel_limit`).
pub const MAX_STRING_BYTES: usize = 64 * 1024;
/// Maximum nesting depth of a bound variable value (`cel_limit`).
pub const MAX_VALUE_DEPTH: usize = 16;
/// Largest integer magnitude a bound variable may carry (2^53 - 1), so both engines read the same
/// number from JSON (`cel_limit`).
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
/// Maximum regex pattern length in UTF-8 bytes (`rule_param_invalid` for the `pattern` rule).
pub const MAX_PATTERN_BYTES: usize = 512;
/// Largest bound in a `{n,m}` quantifier (`pattern_unsupported`).
pub const MAX_REPEAT: u32 = 1000;
/// Size budget of an expanded pattern (`pattern_unsupported`); see `regex_subset`.
pub const MAX_REPEAT_UNITS: u64 = 4500;
/// Size units of a class, class escape or `.` (a literal is 1).
pub const CLASS_UNITS: u64 = 10;
/// Compiled regex size limit in bytes on the server.
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;
/// Largest `min_length` / `max_length` bound.
pub const MAX_LENGTH_BOUND: u64 = 1_048_576;
/// Number of values an `enum` rule may list.
pub const MAX_ENUM_VALUES: usize = 256;
/// Earliest timestamp (0001-01-01T00:00:00Z) in Unix seconds.
pub const MIN_TIMESTAMP: i64 = -62_135_596_800;
/// Latest timestamp (9999-12-31T23:59:59Z) in Unix seconds.
pub const MAX_TIMESTAMP: i64 = 253_402_300_799;
