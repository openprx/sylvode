/**
 * Public field renderers (contract C2 outlets `SiteRead` / `SiteSubmit`). The types below are the
 * MVP set; `member`, `relation`, `child_table`, files, images, `signature`, `location` and `scan`
 * are `Reject` on the site and render nothing (never a placeholder that leaks structure).
 */

const TEXT = new Set(["text", "phone", "email", "address"]);
const AREA = new Set(["textarea", "rich_text"]);
const NUMERIC = new Set(["number", "integer", "amount", "rating", "progress"]);
const READ_ONLY = new Set(["autonumber"]);
const BLOCKED = new Set([
  "member",
  "relation",
  "child_table",
  "attachment",
  "image",
  "signature",
  "location",
  "scan",
]);

export type InputKind =
  | "text"
  | "textarea"
  | "number"
  | "date"
  | "datetime"
  | "single_select"
  | "multi_select"
  | "boolean";

/** The editor a type uses, or null when the type is blocked or read-only on the site. */
export function inputKind(type: string): InputKind | null {
  if (TEXT.has(type)) return "text";
  if (AREA.has(type)) return "textarea";
  if (NUMERIC.has(type)) return "number";
  if (type === "date") return "date";
  if (type === "datetime") return "datetime";
  if (type === "single_select" || type === "select") return "single_select";
  if (type === "multi_select") return "multi_select";
  if (type === "boolean") return "boolean";
  return null;
}

export function isBlockedType(type: string): boolean {
  return BLOCKED.has(type) || (!READ_ONLY.has(type) && inputKind(type) === null);
}

export function isEditableType(type: string): boolean {
  return inputKind(type) !== null;
}

export function isNumericType(type: string): boolean {
  return NUMERIC.has(type);
}
