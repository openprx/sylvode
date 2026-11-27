/** Vendored from frontend/src/lib/forms/rules/cel-subset.ts (rev of branch next 6d5b6b91); keep in sync, do not edit here. */
/**
 * Pre-parse checks and the subset walk over the cel-js tree; mirror of
 * `crates/sv-rules/src/expr/subset.rs`. Nodes are counted at source level the way the Rust walk
 * counts the `cel` crate's tree, so both sides apply the node and cost limits to the same numbers.
 */

import type { ASTNode } from "@marcbachmann/cel-js";
import {
  COMPREHENSION_COST_FACTOR,
  MAX_AST_NODES,
  MAX_BRACKET_DEPTH,
  MAX_CONDITIONALS,
  MAX_SOURCE_BYTES,
  MAX_STATIC_COST,
  utf8Length,
} from "./limits";
import { checkPattern } from "./regex-subset";

export type CompileCode =
  | "cel_too_long"
  | "cel_too_deep"
  | "cel_syntax"
  | "cel_not_in_subset"
  | "pattern_unsupported"
  | "pattern_invalid"
  | "cel_unknown_variable"
  | "cel_too_complex"
  | "cel_result_type";

const TYPE_NAMES = new Set([
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
]);

/** Global functions of the subset and their arity. */
export const GLOBAL_FUNCTIONS: ReadonlyMap<string, number> = new Map([
  ["size", 1],
  ["int", 1],
  ["double", 1],
  ["string", 1],
  ["lower", 1],
  ["upper", 1],
  ["trim", 1],
  ["is_empty", 1],
  ["decimal_cmp", 2],
  ["date", 1],
  ["days_between", 2],
]);

/** Receiver-style functions of the subset and their arity (without the receiver). */
export const MEMBER_FUNCTIONS: ReadonlyMap<string, number> = new Map([
  ["contains", 1],
  ["startsWith", 1],
  ["endsWith", 1],
  ["matches", 1],
  ["getFullYear", 0],
  ["getMonth", 0],
  ["getDate", 0],
  ["getDayOfWeek", 0],
  ["getHours", 0],
]);

const BINARY = new Set([
  "&&",
  "||",
  "+",
  "-",
  "*",
  "/",
  "%",
  "==",
  "!=",
  "<",
  "<=",
  ">",
  ">=",
  "in",
  "[]",
]);

/** Length and bracket checks that run before the parser. */
export function precheck(src: string): CompileCode | null {
  if (utf8Length(src) > MAX_SOURCE_BYTES) return "cel_too_long";
  let depth = 0;
  let conditionals = 0;
  for (const c of src) {
    if (c === "(" || c === "[" || c === "{") {
      if (++depth > MAX_BRACKET_DEPTH) return "cel_too_deep";
    } else if (c === ")" || c === "]" || c === "}") {
      depth = Math.max(0, depth - 1);
    } else if (c === "?") {
      if (++conditionals > MAX_CONDITIONALS) return "cel_too_deep";
    }
  }
  return null;
}

/** cel-js reports a bad `has()` argument only at evaluation; the Rust parser refuses it. */
export function hasBadHas(node: ASTNode): boolean {
  let bad = false;
  visit(node, (n) => {
    if (n.op === "call" && n.args[0] === "has") {
      const args = n.args[1];
      if (args.length !== 1 || args[0]?.op !== ".") bad = true;
    }
  });
  return bad;
}

function children(n: ASTNode): ASTNode[] {
  switch (n.op) {
    case "value":
    case "id":
      return [];
    case ".":
    case ".?":
      return [n.args[0]];
    case "!_":
    case "-_":
      return [n.args];
    case "call":
      return n.args[1];
    case "rcall":
      return [n.args[1], ...n.args[2]];
    case "list":
      return n.args;
    case "map":
      return n.args.flat();
    default:
      return n.args as ASTNode[];
  }
}

function visit(n: ASTNode, f: (n: ASTNode) => void): void {
  f(n);
  for (const c of children(n)) visit(c, f);
}

function isNumericLiteral(n: ASTNode): boolean {
  return n.op === "value" && (typeof n.args === "bigint" || typeof n.args === "number");
}

/**
 * `-<number>` with only white space between: the Rust parser reads it as one literal. With
 * parentheses (`-(5)`) it is a negation there, so the source text decides.
 */
function isFoldedNegative(n: ASTNode): boolean {
  if (n.op !== "-_" || !isNumericLiteral(n.args)) return false;
  return n.input.slice(n.start + 1, n.args.start).trim() === "";
}

/** A literal as the Rust parser sees it. */
function isLiteral(n: ASTNode): boolean {
  return n.op === "value" || isFoldedNegative(n);
}

/** Length of a run of the same unary operator starting at `n`, and the operand under it. */
function unaryRun(n: ASTNode & { op: "!_" | "-_" }): { length: number; operand: ASTNode } {
  let length = 0;
  let cur: ASTNode = n;
  while (cur.op === n.op) {
    length++;
    cur = (cur as ASTNode & { op: "!_" | "-_" }).args;
  }
  return { length, operand: cur };
}

function isPath(n: ASTNode): boolean {
  switch (n.op) {
    case "id":
      return true;
    case ".":
      return isPath(n.args[0]);
    case "[]":
      return isPath(n.args[0]) && isLiteral(n.args[1]);
    default:
      return false;
  }
}

interface Found {
  notInSubset: boolean;
  patternUnsupported: boolean;
  patternInvalid: boolean;
  unknownVariable: boolean;
}

export interface Checked {
  nodes: number;
  cost: number;
}

class Walk {
  private scope: string | null = null;
  private inPredicate = false;
  nodes = 0;
  cost = 0;
  readonly found: Found = {
    notInSubset: false,
    patternUnsupported: false,
    patternInvalid: false,
    unknownVariable: false,
  };

  constructor(private readonly variables: readonly string[]) {}

  private count(n: number): void {
    this.nodes += n;
    this.cost += n * (this.inPredicate ? COMPREHENSION_COST_FACTOR : 1);
  }

  private refuse(): void {
    this.found.notInSubset = true;
  }

  private ident(name: string): void {
    if (this.scope === name || this.variables.includes(name)) return;
    if (TYPE_NAMES.has(name)) this.refuse();
    else this.found.unknownVariable = true;
  }

  expr(n: ASTNode): void {
    switch (n.op) {
      case "value": {
        this.count(1);
        const v = n.args;
        const ok =
          v === null ||
          typeof v === "bigint" ||
          typeof v === "number" ||
          typeof v === "string" ||
          typeof v === "boolean";
        if (!ok) this.refuse();
        return;
      }
      case "id":
        this.count(1);
        this.ident(n.args);
        return;
      case "list":
        this.count(1);
        for (const el of n.args) this.expr(el);
        return;
      case "map":
        this.count(1);
        for (const [k, v] of n.args) {
          this.expr(k);
          this.expr(v);
        }
        return;
      case ".":
        this.count(1);
        this.expr(n.args[0]);
        return;
      case ".?":
      case "[?]":
        this.count(1);
        this.refuse();
        for (const c of children(n)) this.expr(c);
        return;
      case "!_":
      case "-_": {
        // The Rust parser drops an even run of one unary operator and folds `-<number>` into
        // the literal, so the run counts as zero or one node. Any other unary minus is
        // refused: the Rust interpreter negates integers without an overflow check.
        const { length, operand } = unaryRun(n);
        const folded = length === 1 && isFoldedNegative(n);
        if (length % 2 === 1 && !folded) {
          this.count(1);
          if (n.op === "-_") this.refuse();
        }
        this.expr(operand);
        return;
      }
      case "?:":
        this.count(1);
        for (const c of n.args) this.expr(c);
        return;
      case "call":
        this.call(n.args[0], null, n.args[1]);
        return;
      case "rcall":
        this.rcall(n.args[0], n.args[1], n.args[2]);
        return;
      default:
        this.count(1);
        if (!BINARY.has(n.op)) this.refuse();
        for (const c of children(n)) this.expr(c);
    }
  }

  private call(name: string, target: ASTNode | null, args: ASTNode[]): void {
    this.count(1);
    if (name === "has" && target === null) {
      // `has(x.f)`: one node for `has`, one for the select (counted by the walk below). The
      // operand must be a variable path, as on the Rust side.
      const sel = args[0];
      if (sel?.op !== "." || !isPath(sel.args[0])) this.refuse();
      for (const a of args) this.expr(a);
      return;
    }
    const table = target === null ? GLOBAL_FUNCTIONS : MEMBER_FUNCTIONS;
    if (table.get(name) !== args.length) this.refuse();
    if (name === "matches" && target !== null) {
      const p = args[0];
      if (p?.op === "value" && typeof p.args === "string") {
        const code = checkPattern(p.args);
        if (code === "pattern_unsupported") this.found.patternUnsupported = true;
        else if (code !== null) this.found.patternInvalid = true;
      } else {
        this.refuse();
      }
    }
    if (target !== null) this.expr(target);
    for (const a of args) this.expr(a);
  }

  private rcall(name: string, target: ASTNode, args: ASTNode[]): void {
    const isMacro =
      (name === "all" || name === "exists") && args.length === 2 && args[0]?.op === "id";
    if (!isMacro) {
      if (["map", "filter", "exists_one", "existsOne", "all", "exists"].includes(name)) {
        this.count(2);
        this.refuse();
        return;
      }
      this.call(name, target, args);
      return;
    }
    // `range.all(x, p)`: one node for the macro, one for `x`.
    this.count(2);
    if (this.inPredicate) this.refuse();
    if (target.op !== "list" && !isPath(target)) this.refuse();
    const v = args[0]?.args as string;
    if (this.variables.includes(v) || TYPE_NAMES.has(v)) this.refuse();
    this.expr(target);
    const outerScope = this.scope;
    const outerPredicate = this.inPredicate;
    this.scope = v;
    this.inPredicate = true;
    this.expr(args[1] as ASTNode);
    this.inPredicate = outerPredicate;
    this.scope = outerScope;
  }
}

function mayBeBool(root: ASTNode): boolean {
  switch (root.op) {
    case "value":
      return typeof root.args === "boolean";
    case "list":
    case "map":
      return false;
    case "-_":
      return !isFoldedNegative(root);
    default:
      return true;
  }
}

/** Walks a parsed tree; returns the counts or the refusal code (same priority as the Rust walk). */
export function walk(
  root: ASTNode,
  variables: readonly string[],
  boolResult: boolean,
): Checked | CompileCode {
  const w = new Walk(variables);
  w.expr(root);
  const f = w.found;
  if (f.notInSubset) return "cel_not_in_subset";
  if (f.patternUnsupported) return "pattern_unsupported";
  if (f.patternInvalid) return "pattern_invalid";
  if (f.unknownVariable) return "cel_unknown_variable";
  if (w.nodes > MAX_AST_NODES || w.cost > MAX_STATIC_COST) return "cel_too_complex";
  if (boolResult && !mayBeBool(root)) return "cel_result_type";
  return { nodes: w.nodes, cost: w.cost };
}
