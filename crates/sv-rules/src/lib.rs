//! Form rule engine for Sylvode form schema v2.
//!
//! The crate has no dependency on the API server. It provides:
//!
//! - built-in validation rules (`required` is checked by the caller; `min`, `max`, `min_length`,
//!   `max_length`, `pattern`, `enum`, `email`, `url` are parsed and checked here), see [`parse_rule`]
//!   and [`check_builtin`];
//! - the CEL subset: a pre-parse length and bracket-depth check, the `cel` crate parser, a walk of
//!   the parsed tree that refuses anything outside the subset, and an evaluator with bounded inputs,
//!   see [`compile`] and [`Program`];
//! - the regex subset shared by the server and the browser, see [`check_pattern`] and
//!   [`compile_pattern`].
//!
//! The TypeScript mirror lives in `frontend/src/lib/forms/rules/`. Both sides run the shared
//! conformance fixtures in `fixtures/conformance.json` (`cargo run -p sv-rules --example
//! conformance -- <file>`), and their outputs must be identical.
//!
//! Every limit is a constant in [`limits`]; the TypeScript mirror uses the same numbers.

mod builtin;
mod decimal;
mod error;
mod expr;
pub mod limits;
mod regex_subset;
mod text;

pub use builtin::{
    Bound, BuiltinRule, Operand, PluginRuleRef, RuleKind, RuleSpec, RuleSpecKind, check_builtin, parse_rule,
};
pub use decimal::compare_canonical_decimal;
pub use error::{RuleDefError, RuleEvalError};
pub use expr::{Env, Program, Vars, compile};
pub use regex_subset::{check_pattern, compile_pattern};
pub use text::{is_email, is_url};
