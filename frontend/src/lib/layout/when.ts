/**
 * Display-only visibility for a node `when` or a placement `condition`.
 *
 * The server decides access on every read and action; this only hides or shows. The CEL
 * environment `layout_when` is not available in the console yet: it arrives with the shared
 * rules mirror under `frontend/src/lib/forms/rules/`, and this function is its single call site to
 * switch over. Until then only the literals `true` and `false` are evaluated; every other
 * expression is treated as visible.
 */
export function evaluateWhen(expression: string | null | undefined): boolean {
	if (typeof expression !== 'string') return true;
	return expression.trim().toLowerCase() !== 'false';
}
