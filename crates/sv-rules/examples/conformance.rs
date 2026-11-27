//! Conformance runner: `cargo run -p sv-rules --example conformance -- <fixtures.json>`.
//!
//! Prints `<id> <verdict>` per case; the bun runner (`frontend/scripts/cel-conformance.ts`) prints
//! the same lines, and `diff` of the two outputs must be empty. Cases whose verdict differs from
//! the fixture's `expect` are listed on stderr and make the exit status 1.
//!
//! Verdicts: `value=<canonical>`, `compile_error=<code>`, `eval_error=<code>` for expressions;
//! `pass`, `fail=<code>`, `def_error=<code>@<path>` for built-in rules.

use std::io::Write as _;
use std::process::ExitCode;

use rust_decimal::Decimal;
use serde_json::{Map, Value};
use sv_rules::{Env, Operand, RuleKind, RuleSpecKind, Vars, check_builtin, compile, parse_rule};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

fn js_number(f: f64) -> String {
    // Same text as the bun runner: ECMAScript Number::toString through the CEL `string()` function.
    let program = compile("string(value)", Env::FieldRule);
    let vars = Vars::new().with("value", Value::from(f));
    match program.ok().and_then(|p| p.eval(&vars).ok()) {
        Some(cel::Value::String(s)) => s.to_string(),
        _ => f.to_string(),
    }
}

fn canonical_cel(v: &cel::Value) -> String {
    match v {
        cel::Value::Null => "null".to_owned(),
        cel::Value::Bool(b) => b.to_string(),
        cel::Value::Int(i) => format!("int:{i}"),
        cel::Value::UInt(u) => format!("uint:{u}"),
        cel::Value::Float(f) => format!("double:{}", float_text(*f)),
        cel::Value::String(s) => format!("string:{}", Value::from(s.as_str())),
        cel::Value::List(items) => {
            let inner: Vec<String> = items.iter().map(canonical_cel).collect();
            format!("[{}]", inner.join(","))
        }
        cel::Value::Map(m) => {
            let mut entries: Vec<String> = m
                .map
                .iter()
                .map(|(k, v)| format!("{}:{}", canonical_cel(&cel::Value::from(k)), canonical_cel(v)))
                .collect();
            entries.sort();
            format!("{{{}}}", entries.join(","))
        }
        other => format!("unsupported:{other:?}"),
    }
}

fn float_text(f: f64) -> String {
    if f.is_nan() { "NaN".to_owned() } else { js_number(f) }
}

/// The canonical text of an expected JSON value, read with the binding rule (integral numbers
/// within ±(2^53 - 1) are ints).
fn canonical_json(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => format!("int:{i}"),
            (None, Some(f)) if f.fract() == 0.0 && f.abs() <= MAX_SAFE => format!("int:{f}"),
            (None, Some(f)) => format!("double:{}", float_text(f)),
            _ => format!("number:{n}"),
        },
        Value::String(s) => format!("string:{}", Value::from(s.as_str())),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        Value::Object(m) => {
            let mut entries: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("string:{}:{}", Value::from(k.as_str()), canonical_json(v)))
                .collect();
            entries.sort();
            format!("{{{}}}", entries.join(","))
        }
    }
}

fn expr_case(case: &Map<String, Value>) -> (String, String) {
    let env = case
        .get("env")
        .and_then(Value::as_str)
        .and_then(Env::from_name)
        .unwrap_or(Env::FieldRule);
    let src = case.get("expr").and_then(Value::as_str).unwrap_or_default();
    let vars = Vars::from_object(case.get("vars").and_then(Value::as_object).cloned().unwrap_or_default());
    let got = match compile(src, env) {
        Err(e) => format!("compile_error={}", e.code),
        Ok(p) => match p.eval(&vars) {
            Ok(v) => format!("value={}", canonical_cel(&v)),
            Err(e) => format!("eval_error={}", e.code),
        },
    };
    let expect = case.get("expect").and_then(Value::as_object);
    let want = match expect {
        Some(e) if e.contains_key("value") => format!("value={}", canonical_json(&e["value"])),
        Some(e) if e.contains_key("compile_error") => {
            format!("compile_error={}", e["compile_error"].as_str().unwrap_or_default())
        }
        Some(e) if e.contains_key("eval_error") => {
            format!("eval_error={}", e["eval_error"].as_str().unwrap_or_default())
        }
        _ => "missing-expect".to_owned(),
    };
    (got, want)
}

fn operand(v: Option<&Value>) -> Option<Operand> {
    let (kind, value) = v?.as_object()?.iter().next()?;
    Some(match kind.as_str() {
        "int" => Operand::Int(value.as_i64()?),
        "decimal" => Operand::Decimal(value.as_str()?.parse::<Decimal>().ok()?),
        "date" => Operand::Date(
            time::Date::parse(
                value.as_str()?,
                time::macros::format_description!("[year]-[month]-[day]"),
            )
            .ok()?,
        ),
        "datetime" => Operand::DateTime(OffsetDateTime::parse(value.as_str()?, &Rfc3339).ok()?),
        "str" => Operand::Str(value.as_str()?.to_owned()),
        "count" => Operand::Count(usize::try_from(value.as_u64()?).ok()?),
        _ => return None,
    })
}

fn rule_case(case: &Map<String, Value>) -> (String, String) {
    let allowed: Vec<RuleKind> = case.get("allowed").and_then(Value::as_array).map_or_else(
        || RuleKind::ALL.to_vec(),
        |a| {
            a.iter()
                .filter_map(|k| k.as_str().and_then(RuleKind::from_name))
                .collect()
        },
    );
    let got = match parse_rule(&case["rule"], &allowed) {
        Err(e) => format!("def_error={}@{}", e.code, e.path),
        Ok(spec) => match (spec.kind, operand(case.get("operand"))) {
            (RuleSpecKind::Builtin(rule), Some(op)) => match check_builtin(&rule, &op) {
                Ok(()) => "pass".to_owned(),
                Err(code) => format!("fail={code}"),
            },
            (RuleSpecKind::Builtin(_), None) => "bad-operand".to_owned(),
            _ => "parsed".to_owned(),
        },
    };
    let expect = case.get("expect").and_then(Value::as_object);
    let want = match expect {
        Some(e) if e.contains_key("pass") => "pass".to_owned(),
        Some(e) if e.contains_key("parsed") => "parsed".to_owned(),
        Some(e) if e.contains_key("fail") => format!("fail={}", e["fail"].as_str().unwrap_or_default()),
        Some(e) if e.contains_key("def_error") => format!(
            "def_error={}@{}",
            e["def_error"].as_str().unwrap_or_default(),
            e.get("path").and_then(Value::as_str).unwrap_or_default()
        ),
        _ => "missing-expect".to_owned(),
    };
    (got, want)
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        let _ = writeln!(std::io::stderr(), "usage: conformance <fixtures.json>");
        return ExitCode::from(2);
    };
    let fixtures: Value = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "cannot read {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let cases = fixtures
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let mut mismatches = 0_usize;
    for case in &cases {
        let Some(case) = case.as_object() else { continue };
        let id = case.get("id").and_then(Value::as_str).unwrap_or("?");
        let (got, want) = if case.contains_key("rule") {
            rule_case(case)
        } else {
            expr_case(case)
        };
        let _ = writeln!(out, "{id} {got}");
        if got != want {
            mismatches += 1;
            let _ = writeln!(err, "MISMATCH {id}: got {got}, expected {want}");
        }
    }
    let _ = writeln!(err, "{} cases, {mismatches} differ from expect", cases.len());
    if mismatches == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
