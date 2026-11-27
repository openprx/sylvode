//! Pre-parse checks and the subset walk over the `cel` crate's parsed tree.
//!
//! The TypeScript mirror (`cel-subset.ts`) walks the cel-js tree with the same rules and counts
//! nodes the same way, so both sides give the same compile verdict.

use cel::common::ast::{CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedExpr, LiteralValue, operators};

use crate::limits::{
    COMPREHENSION_COST_FACTOR, MAX_AST_NODES, MAX_BRACKET_DEPTH, MAX_CONDITIONALS, MAX_SOURCE_BYTES, MAX_STATIC_COST,
};
use crate::regex_subset::check_pattern;

/// Type names refused as values (`cel_not_in_subset` instead of `cel_unknown_variable`).
const TYPE_NAMES: &[&str] = &[
    "bool",
    "bytes",
    "double",
    "duration",
    "dyn",
    "int",
    "list",
    "map",
    "null_type",
    "string",
    "timestamp",
    "type",
    "uint",
];

/// Global functions of the subset and their arity.
pub const GLOBAL_FUNCTIONS: &[(&str, usize)] = &[
    ("size", 1),
    ("int", 1),
    ("double", 1),
    ("string", 1),
    ("lower", 1),
    ("upper", 1),
    ("trim", 1),
    ("is_empty", 1),
    ("decimal_cmp", 2),
    ("date", 1),
    ("days_between", 2),
];

/// Receiver-style functions of the subset and their arity (without the receiver).
pub const MEMBER_FUNCTIONS: &[(&str, usize)] = &[
    ("contains", 1),
    ("startsWith", 1),
    ("endsWith", 1),
    ("matches", 1),
    ("getFullYear", 0),
    ("getMonth", 0),
    ("getDate", 0),
    ("getDayOfWeek", 0),
    ("getHours", 0),
];

/// Unary minus is refused on anything but a numeric literal (which the parser folds into the
/// literal): the `cel` crate negates integers without an overflow check.
const UNARY_OPERATORS: &[&str] = &[operators::LOGICAL_NOT];
const BINARY_OPERATORS: &[&str] = &[
    operators::LOGICAL_AND,
    operators::LOGICAL_OR,
    operators::ADD,
    operators::SUBSTRACT,
    operators::MULTIPLY,
    operators::DIVIDE,
    operators::MODULO,
    operators::EQUALS,
    operators::NOT_EQUALS,
    operators::LESS,
    operators::LESS_EQUALS,
    operators::GREATER,
    operators::GREATER_EQUALS,
    operators::IN,
    operators::INDEX,
];

/// Length and bracket checks that run before the parser.
pub fn precheck(src: &str) -> Result<(), &'static str> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err("cel_too_long");
    }
    let mut depth: usize = 0;
    let mut conditionals: usize = 0;
    for b in src.bytes() {
        match b {
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > MAX_BRACKET_DEPTH {
                    return Err("cel_too_deep");
                }
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'?' => {
                conditionals += 1;
                if conditionals > MAX_CONDITIONALS {
                    return Err("cel_too_deep");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Errors found by the walk, reported by priority so the verdict does not depend on the order of
/// traversal.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "four independent error flags, reported by priority"
)]
struct Found {
    not_in_subset: bool,
    pattern_unsupported: bool,
    pattern_invalid: bool,
    unknown_variable: bool,
}

pub struct Walk<'a> {
    variables: &'a [&'a str],
    scope: Option<String>,
    in_predicate: bool,
    nodes: usize,
    cost: u64,
    found: Found,
}

/// The result of a successful walk.
pub struct Checked {
    pub nodes: usize,
    pub cost: u64,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(variables: &'a [&'a str]) -> Self {
        Self {
            variables,
            scope: None,
            in_predicate: false,
            nodes: 0,
            cost: 0,
            found: Found::default(),
        }
    }

    const fn count(&mut self, n: usize) {
        self.nodes += n;
        let weight = if self.in_predicate {
            COMPREHENSION_COST_FACTOR
        } else {
            1
        };
        self.cost = self.cost.saturating_add(weight * n as u64);
    }

    const fn refuse(&mut self) {
        self.found.not_in_subset = true;
    }

    pub(crate) fn run(mut self, root: &IdedExpr, bool_result: bool) -> Result<Checked, &'static str> {
        self.expr(root);
        let found = &self.found;
        if found.not_in_subset {
            return Err("cel_not_in_subset");
        }
        if found.pattern_unsupported {
            return Err("pattern_unsupported");
        }
        if found.pattern_invalid {
            return Err("pattern_invalid");
        }
        if found.unknown_variable {
            return Err("cel_unknown_variable");
        }
        if self.nodes > MAX_AST_NODES || self.cost > MAX_STATIC_COST {
            return Err("cel_too_complex");
        }
        if bool_result && !may_be_bool(root) {
            return Err("cel_result_type");
        }
        Ok(Checked {
            nodes: self.nodes,
            cost: self.cost,
        })
    }

    fn expr(&mut self, e: &IdedExpr) {
        match &e.expr {
            Expr::Literal(lit) => {
                self.count(1);
                if matches!(lit, LiteralValue::Bytes(_) | LiteralValue::UInt(_)) {
                    self.refuse();
                }
            }
            Expr::Ident(name) => {
                self.count(1);
                self.ident(name);
            }
            Expr::List(list) => {
                self.count(1);
                if !list.optional_indices.is_empty() {
                    self.refuse();
                }
                for el in &list.elements {
                    self.expr(el);
                }
            }
            Expr::Map(map) => {
                self.count(1);
                for entry in &map.entries {
                    match &entry.expr {
                        EntryExpr::MapEntry(me) => {
                            if me.optional {
                                self.refuse();
                            }
                            self.expr(&me.key);
                            self.expr(&me.value);
                        }
                        EntryExpr::StructField(_) => self.refuse(),
                    }
                }
            }
            Expr::Select(sel) => {
                // `has(x.f)` is a select with `test` set: one node for `has`, one for the select.
                // Its operand must be a variable path; the engines disagree on `has` over literals.
                self.count(if sel.test { 2 } else { 1 });
                if sel.test && !is_path(&sel.operand) {
                    self.refuse();
                }
                self.expr(&sel.operand);
            }
            Expr::Call(call) => self.call(call),
            Expr::Comprehension(comp) => self.comprehension(comp),
            Expr::Struct(_) | Expr::Unspecified => {
                self.count(1);
                self.refuse();
            }
        }
    }

    fn ident(&mut self, name: &str) {
        if self.scope.as_deref() == Some(name) || self.variables.contains(&name) {
            return;
        }
        if name.starts_with('@') || TYPE_NAMES.contains(&name) {
            self.refuse();
        } else {
            self.found.unknown_variable = true;
        }
    }

    fn call(&mut self, call: &CallExpr) {
        self.count(1);
        let name = call.func_name.as_str();
        let arity = call.args.len();
        let allowed = match &call.target {
            None if name == operators::CONDITIONAL => arity == 3,
            None if UNARY_OPERATORS.contains(&name) => arity == 1,
            None if BINARY_OPERATORS.contains(&name) => arity == 2,
            None => GLOBAL_FUNCTIONS.contains(&(name, arity)),
            Some(_) => MEMBER_FUNCTIONS.contains(&(name, arity)),
        };
        if !allowed {
            self.refuse();
        }
        if name == "matches" && call.target.is_some() {
            match call.args.first().map(|a| &a.expr) {
                Some(Expr::Literal(LiteralValue::String(p))) => match check_pattern(p.inner()) {
                    Ok(()) => {}
                    Err("pattern_unsupported") => self.found.pattern_unsupported = true,
                    Err(_) => self.found.pattern_invalid = true,
                },
                _ => self.refuse(),
            }
        }
        if let Some(target) = &call.target {
            self.expr(target);
        }
        for arg in &call.args {
            self.expr(arg);
        }
    }

    fn comprehension(&mut self, comp: &ComprehensionExpr) {
        // `range.all(x, p)` / `range.exists(x, p)`: one node for the macro, one for `x`.
        self.count(2);
        let Some(predicate) = classify(comp) else {
            self.refuse();
            return;
        };
        if self.in_predicate || comp.iter_var2.is_some() {
            self.refuse();
        }
        if !is_range(&comp.iter_range) {
            self.refuse();
        }
        let var = comp.iter_var.as_str();
        if var.starts_with('@') || self.variables.contains(&var) || TYPE_NAMES.contains(&var) {
            self.refuse();
        }
        self.expr(&comp.iter_range);
        let outer_scope = self.scope.replace(comp.iter_var.clone());
        let outer_predicate = std::mem::replace(&mut self.in_predicate, true);
        self.expr(predicate);
        self.in_predicate = outer_predicate;
        self.scope = outer_scope;
    }
}

/// Recognises the trees the parser builds for `all` and `exists`; every other macro is refused.
fn classify(comp: &ComprehensionExpr) -> Option<&IdedExpr> {
    let Expr::Literal(LiteralValue::Boolean(init)) = &comp.accu_init.expr else {
        return None;
    };
    let Expr::Call(step) = &comp.loop_step.expr else {
        return None;
    };
    let is_all = *init.inner() && step.func_name == operators::LOGICAL_AND;
    let is_exists = !*init.inner() && step.func_name == operators::LOGICAL_OR;
    if !is_all && !is_exists {
        return None;
    }
    match step.args.as_slice() {
        [accu, predicate] if matches!(&accu.expr, Expr::Ident(n) if *n == comp.accu_var) => Some(predicate),
        _ => None,
    }
}

/// A macro range is a list literal or a variable path (`a`, `a.b`, `a[0]`, `a.b["k"]`), so the list
/// it iterates is bounded by the AST limit or by the bound-variable limits.
fn is_range(e: &IdedExpr) -> bool {
    matches!(e.expr, Expr::List(_)) || is_path(e)
}

fn is_path(e: &IdedExpr) -> bool {
    match &e.expr {
        Expr::Ident(_) => true,
        Expr::Select(sel) => !sel.test && is_path(&sel.operand),
        Expr::Call(call) if call.func_name == operators::INDEX && call.target.is_none() => {
            matches!(call.args.as_slice(), [base, key] if is_path(base) && matches!(key.expr, Expr::Literal(_)))
        }
        _ => false,
    }
}

/// A shallow result-type check for boolean environments: a non-boolean literal, list or map at the
/// root can never be `true`.
const fn may_be_bool(root: &IdedExpr) -> bool {
    match &root.expr {
        Expr::Literal(lit) => matches!(lit, LiteralValue::Boolean(_)),
        Expr::List(_) | Expr::Map(_) => false,
        _ => true,
    }
}
