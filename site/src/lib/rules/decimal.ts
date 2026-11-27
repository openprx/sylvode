/** Vendored from frontend/src/lib/forms/rules/decimal.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * Exact comparison of canonical decimal strings (`-?[0-9]+(\.[0-9]+)?`, any length); mirror of
 * `crates/sv-rules/src/decimal.rs`, used by CEL `decimal_cmp` and the `min`/`max` rules.
 */

interface Parts {
  negative: boolean;
  int: string;
  frac: string;
}

const CANONICAL = /^(-?)([0-9]+)(?:\.([0-9]+))?$/;

function parse(s: string): Parts | null {
  const m = CANONICAL.exec(s);
  if (!m) return null;
  return {
    negative: m[1] === "-",
    int: (m[2] ?? "").replace(/^0+/, ""),
    frac: (m[3] ?? "").replace(/0+$/, ""),
  };
}

function cmp(a: string, b: string): -1 | 0 | 1 {
  return a < b ? -1 : a > b ? 1 : 0;
}

function compareMagnitude(a: Parts, b: Parts): -1 | 0 | 1 {
  if (a.int.length !== b.int.length) return a.int.length < b.int.length ? -1 : 1;
  return cmp(a.int, b.int) || cmp(a.frac, b.frac);
}

/** Compares two canonical decimals exactly; `null` when either is not canonical. */
export function compareCanonicalDecimal(a: string, b: string): -1 | 0 | 1 | null {
  const pa = parse(a);
  const pb = parse(b);
  if (!pa || !pb) return null;
  const aNeg = pa.negative && (pa.int !== "" || pa.frac !== "");
  const bNeg = pb.negative && (pb.int !== "" || pb.frac !== "");
  if (!aNeg && bNeg) return 1;
  if (aNeg && !bNeg) return -1;
  return aNeg ? compareMagnitude(pb, pa) : compareMagnitude(pa, pb);
}

export function isCanonicalDecimal(s: string): boolean {
  return CANONICAL.test(s);
}
