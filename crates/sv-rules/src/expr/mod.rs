//! The CEL subset: compile (pre-checks, parse, subset walk) and evaluate.
//!
//! Language (contract C2 §5.1): literals int, double, string, bool, null, list and map; the
//! operators `! - * / % + < <= > >= == != && || ?: in`, index and field select; the macros `has`,
//! `all` and `exists` (no nesting; the range is a list literal or a variable path); the functions in
//! `subset::GLOBAL_FUNCTIONS` and `subset::MEMBER_FUNCTIONS`. `matches` takes a string literal from
//! the regex subset. Unary minus applies to numeric literals only (write `0 - x`), because the
//! `cel` crate negates integers without an overflow check; `has()` takes a variable path.
//!
//! Timestamps are integers: Unix seconds, UTC. `now`, datetime field values, `date('YYYY-MM-DD')`
//! and the receiver of the getters (`getFullYear`, `getMonth` (0-based), `getDate` (1-based),
//! `getDayOfWeek` (0 = Sunday), `getHours`) are all `int`, and `days_between(a, b)` is
//! `(b - a) / 86400` truncated. The standard `timestamp()` and `duration()` constructors are not in
//! the subset: they need the `cel` crate's `chrono` feature, which the audit policy denies.
//!
//! Bound values come from JSON. A JSON number with an integral value within ±(2^53 - 1) is an `int`,
//! any other number a `double`; strings are at most 64 KiB, lists and maps at most 256 entries and
//! values at most 16 levels deep; anything else is `cel_limit`.
//!
//! Known differences from the browser (cel-js 8.0.0), all on ill-typed or out-of-range input, and
//! all failing closed on the server: `int()` of a double outside the int64 range (server
//! `cel_error`, browser a value); `has(x.f)` where `x` is not a map (server `cel_error`, browser
//! `false`); an even run of `!` or `-` over a value of the wrong type (the parser drops the run, so
//! the server returns the operand where the browser fails); heterogeneous equality inside lists
//! and maps (`[1] == [1.0]`: server `true`, browser `cel_error`). The interpreter runs inside
//! `catch_unwind`, so a panic in it is `cel_error`.

mod bind;
mod functions;
mod subset;

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use cel::Context;
use cel::common::ast::{Expr, IdedExpr, LiteralValue};
use regex::Regex;

use crate::error::{RuleDefError, RuleEvalError};
use crate::regex_subset::compile_pattern;

/// Where an expression is used; fixes its variables and result type (contract C2 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Env {
    /// `rules.validate[].cel`: `value`, `record`, `now`, `user`; result `bool` (true = pass).
    FieldRule,
    /// `default.cel`: `now`, `user`; result of the field's CEL type.
    Default,
    /// Layout `when` and `{"cel": ..}` props: `params`, `data`, `slot`, `viewer`; `bool` or string.
    LayoutWhen,
}

impl Env {
    /// The variables an expression in this environment may name.
    #[must_use]
    pub const fn variables(self) -> &'static [&'static str] {
        match self {
            Self::FieldRule => &["value", "record", "now", "user"],
            Self::Default => &["now", "user"],
            Self::LayoutWhen => &["params", "data", "slot", "viewer"],
        }
    }

    /// The wire name used in schemas and fixtures.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FieldRule => "field_rule",
            Self::Default => "default",
            Self::LayoutWhen => "layout_when",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "field_rule" => Some(Self::FieldRule),
            "default" => Some(Self::Default),
            "layout_when" => Some(Self::LayoutWhen),
            _ => None,
        }
    }

    const fn bool_result(self) -> bool {
        matches!(self, Self::FieldRule)
    }
}

/// Variable values for one evaluation, as JSON. Names outside the environment are ignored.
#[derive(Clone, Debug, Default)]
pub struct Vars {
    values: BTreeMap<String, serde_json::Value>,
}

impl Vars {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a variable.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: serde_json::Value) -> Self {
        self.insert(name, value);
        self
    }

    /// Adds or replaces a variable.
    pub fn insert(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.values.insert(name.into(), value);
    }

    /// Builds the variables from a JSON object (`{"value": .., "record": {..}}`).
    #[must_use]
    pub fn from_object(object: serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            values: object.into_iter().collect(),
        }
    }
}

/// A checked expression, ready to evaluate. `Send + Sync`, so callers can cache it.
#[derive(Debug)]
pub struct Program {
    compiled: cel::Program,
    env: Env,
    source: String,
    nodes: usize,
    cost: u64,
    patterns: Arc<Vec<(String, Regex)>>,
}

/// Compiles an expression: length and bracket checks, the `cel` parser, the subset walk, the
/// identifier check against `env`, the node and cost limits, and a shallow result-type check.
///
/// # Errors
/// [`RuleDefError`] with an empty path and one of `cel_too_long`, `cel_too_deep`, `cel_syntax`,
/// `cel_not_in_subset`, `pattern_unsupported`, `pattern_invalid`, `cel_unknown_variable`,
/// `cel_too_complex`, `cel_result_type`.
pub fn compile(src: &str, env: Env) -> Result<Program, RuleDefError> {
    subset::precheck(src).map_err(RuleDefError::root)?;
    // The parser is third-party code; a panic in it is a syntax error, not a crash.
    let parsed = std::panic::catch_unwind(|| cel::Program::compile(src));
    let Ok(Ok(program)) = parsed else {
        return Err(RuleDefError::root("cel_syntax"));
    };
    let checked = subset::Walk::new(env.variables())
        .run(program.expression(), env.bool_result())
        .map_err(RuleDefError::root)?;
    let mut patterns = Vec::new();
    collect_patterns(program.expression(), &mut patterns)?;
    Ok(Program {
        compiled: program,
        env,
        source: src.to_owned(),
        nodes: checked.nodes,
        cost: checked.cost,
        patterns: Arc::new(patterns),
    })
}

fn collect_patterns(e: &IdedExpr, out: &mut Vec<(String, Regex)>) -> Result<(), RuleDefError> {
    match &e.expr {
        Expr::Call(call) => {
            if call.func_name == "matches"
                && call.target.is_some()
                && let Some(Expr::Literal(LiteralValue::String(p))) = call.args.first().map(|a| &a.expr)
            {
                let p = p.inner();
                if !out.iter().any(|(known, _)| known == p) {
                    let re = compile_pattern(p, false).map_err(RuleDefError::root)?;
                    out.push((p.to_owned(), re));
                }
            }
            if let Some(t) = &call.target {
                collect_patterns(t, out)?;
            }
            for a in &call.args {
                collect_patterns(a, out)?;
            }
        }
        Expr::Comprehension(c) => {
            collect_patterns(&c.iter_range, out)?;
            collect_patterns(&c.loop_step, out)?;
        }
        Expr::List(l) => {
            for el in &l.elements {
                collect_patterns(el, out)?;
            }
        }
        Expr::Map(m) => {
            for entry in &m.entries {
                if let cel::common::ast::EntryExpr::MapEntry(me) = &entry.expr {
                    collect_patterns(&me.key, out)?;
                    collect_patterns(&me.value, out)?;
                }
            }
        }
        Expr::Select(s) => collect_patterns(&s.operand, out)?,
        Expr::Ident(_) | Expr::Literal(_) | Expr::Struct(_) | Expr::Unspecified => {}
    }
    Ok(())
}

impl Program {
    /// The environment the program was compiled for.
    #[must_use]
    pub const fn env(&self) -> Env {
        self.env
    }

    /// The source text.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Source-level AST node count (the number checked against the node limit).
    #[must_use]
    pub const fn nodes(&self) -> usize {
        self.nodes
    }

    /// Static cost (the number checked against the cost limit).
    #[must_use]
    pub const fn cost(&self) -> u64 {
        self.cost
    }

    /// Evaluates the program.
    ///
    /// # Errors
    /// `cel_limit` when a bound value is over a limit; `cel_error` for any evaluation error.
    pub fn eval(&self, vars: &Vars) -> Result<cel::Value, RuleEvalError> {
        let mut ctx = Context::with_env(Arc::new(cel::Env::default()));
        functions::register(&mut ctx, Arc::clone(&self.patterns));
        for name in self.env.variables() {
            if let Some(v) = vars.values.get(*name) {
                ctx.add_variable_from_value(*name, bind::to_value(v)?);
            }
        }
        // The interpreter is third-party code; a panic in it fails the rule closed.
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| self.compiled.execute(&ctx)));
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) | Err(_) => Err(RuleEvalError::error()),
        }
    }

    /// Evaluates a boolean program (the `field_rule` and `layout_when` `when` result).
    ///
    /// # Errors
    /// As [`Program::eval`]; `cel_error` also when the result is not a boolean.
    pub fn eval_bool(&self, vars: &Vars) -> Result<bool, RuleEvalError> {
        match self.eval(vars)? {
            cel::Value::Bool(b) => Ok(b),
            _ => Err(RuleEvalError::error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn verdict(src: &str, env: Env, vars: &Vars) -> String {
        match compile(src, env) {
            Err(e) => format!("compile:{}", e.code),
            Ok(p) => match p.eval(vars) {
                Ok(v) => format!("{v:?}"),
                Err(e) => format!("eval:{}", e.code),
            },
        }
    }

    #[test]
    fn compiles_and_evaluates_the_subset() {
        let vars = Vars::new()
            .with("value", json!("abc"))
            .with("record", json!({"total": "99.5", "tags": ["a", "b"]}));
        assert_eq!(verdict("size(value) <= 16", Env::FieldRule, &vars), "Bool(true)");
        assert_eq!(
            verdict("decimal_cmp(record.total, '100.00') < 0", Env::FieldRule, &vars),
            "Bool(true)"
        );
        assert_eq!(
            verdict("record.tags.all(t, size(t) == 1)", Env::FieldRule, &vars),
            "Bool(true)"
        );
        assert_eq!(
            verdict("value.matches('^[a-c]+$')", Env::FieldRule, &vars),
            "Bool(true)"
        );
    }

    #[test]
    fn refuses_outside_the_subset() {
        let vars = Vars::new();
        assert_eq!(
            verdict("[1,2].map(x, x)", Env::FieldRule, &vars),
            "compile:cel_not_in_subset"
        );
        assert_eq!(
            verdict("secret == 1", Env::FieldRule, &vars),
            "compile:cel_unknown_variable"
        );
        assert_eq!(
            verdict("value == 1", Env::Default, &vars),
            "compile:cel_unknown_variable"
        );
        assert_eq!(
            verdict(&format!("'{}' == value", "a".repeat(1100)), Env::FieldRule, &vars),
            "compile:cel_too_long"
        );
        assert_eq!(
            verdict(&format!("{}1{}", "(".repeat(25), ")".repeat(25)), Env::FieldRule, &vars),
            "compile:cel_too_deep"
        );
        assert_eq!(
            verdict("value.matches(value)", Env::FieldRule, &vars),
            "compile:cel_not_in_subset"
        );
        assert_eq!(
            verdict("value.matches('\\\\bx')", Env::FieldRule, &vars),
            "compile:pattern_unsupported"
        );
        let sum = compile("1 + 1", Env::FieldRule).map(|p| p.eval_bool(&vars));
        assert_eq!(sum.ok(), Some(Err(RuleEvalError::error())));
        assert_eq!(verdict("'a'", Env::FieldRule, &vars), "compile:cel_result_type");
    }

    #[test]
    fn deep_unary_and_conditional_chains_do_not_overflow() {
        let nots = format!("{}true", "!".repeat(1000));
        assert_eq!(compile(&nots, Env::FieldRule).map(|p| p.nodes()).ok(), Some(1));
        let chain = format!("{}true", "true ? true : ".repeat(24));
        assert!(compile(&chain, Env::FieldRule).is_ok());
        let chain = format!("{}true", "true ? true : ".repeat(25));
        assert_eq!(
            compile(&chain, Env::FieldRule).err().map(|e| e.code),
            Some("cel_too_deep")
        );
    }

    #[test]
    fn bound_values_are_limited() {
        let program = compile("size(value) > 0", Env::FieldRule);
        let long = Vars::new().with("value", json!("x".repeat(64 * 1024 + 1)));
        assert_eq!(
            program.as_ref().map(|p| p.eval(&long)).ok(),
            Some(Err(RuleEvalError::limit()))
        );
        let list = Vars::new().with("value", json!(vec![1; 257]));
        assert_eq!(program.map(|p| p.eval(&list)).ok(), Some(Err(RuleEvalError::limit())));
    }
}
