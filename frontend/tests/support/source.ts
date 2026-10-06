/**
 * Source text with its comments removed, for the few checks that read a Svelte page's wiring.
 *
 * A check that matches a call in the raw source also matches the same text left behind in a
 * comment, so a page whose handler was replaced (and the old line commented out) would still pass.
 * Line comments, block comments and HTML comments are removed first; string contents are kept as
 * they are, which is good enough for the call shapes these checks look for.
 */
export function withoutComments(source: string): string {
	return source
		.replace(/<!--[\s\S]*?-->/g, '')
		.replace(/\/\*[\s\S]*?\*\//g, '')
		.replace(/(^|[^:'"`\\])\/\/[^\n]*/g, '$1');
}
