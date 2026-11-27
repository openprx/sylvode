//! Exact comparison of canonical decimal strings, shared by the CEL `decimal_cmp` function and the
//! TypeScript comparator (`decimal.ts`). Grammar: `-?[0-9]+(\.[0-9]+)?`; any length.

use std::cmp::Ordering;

struct Parts<'a> {
    negative: bool,
    int: &'a str,
    frac: &'a str,
}

fn parse(s: &str) -> Option<Parts<'_>> {
    let (negative, body) = s.strip_prefix('-').map_or((false, s), |rest| (true, rest));
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => {
            if f.is_empty() {
                return None;
            }
            (i, f)
        }
        None => (body, ""),
    };
    if int.is_empty() || !int.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    Some(Parts { negative, int, frac })
}

fn compare_magnitude(a: &Parts<'_>, b: &Parts<'_>) -> Ordering {
    a.int
        .len()
        .cmp(&b.int.len())
        .then_with(|| a.int.cmp(b.int))
        .then_with(|| a.frac.cmp(b.frac))
}

/// Compares two canonical decimal strings exactly. `None` when either is not a canonical decimal.
#[must_use]
pub fn compare_canonical_decimal(a: &str, b: &str) -> Option<Ordering> {
    let a = parse(a)?;
    let b = parse(b)?;
    let a_zero = a.int.is_empty() && a.frac.is_empty();
    let b_zero = b.int.is_empty() && b.frac.is_empty();
    let a_neg = a.negative && !a_zero;
    let b_neg = b.negative && !b_zero;
    Some(match (a_neg, b_neg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => compare_magnitude(&a, &b),
        (true, true) => compare_magnitude(&b, &a),
    })
}

/// Whether `s` matches the canonical decimal grammar.
pub fn is_canonical_decimal(s: &str) -> bool {
    parse(s).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_by_value() {
        assert_eq!(compare_canonical_decimal("99.5", "100.00"), Some(Ordering::Less));
        assert_eq!(compare_canonical_decimal("1.50", "1.5"), Some(Ordering::Equal));
        assert_eq!(compare_canonical_decimal("-0", "0.0"), Some(Ordering::Equal));
        assert_eq!(compare_canonical_decimal("-2", "-10"), Some(Ordering::Greater));
        assert_eq!(compare_canonical_decimal("007", "7"), Some(Ordering::Equal));
        assert_eq!(compare_canonical_decimal("1.", "1"), None);
        assert_eq!(compare_canonical_decimal("1e3", "1"), None);
        assert_eq!(compare_canonical_decimal("+1", "1"), None);
    }
}
