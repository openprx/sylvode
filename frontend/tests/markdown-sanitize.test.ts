/**
 * Issue, comment and proposal markdown is rendered with `{@html}`. Raw HTML must stay escaped.
 * A previous `<video>` passthrough re-inserted the matched source unchanged.
 *
 * Run standalone: `bun tests/markdown-sanitize.test.ts`
 */

import { renderMarkdown } from '../src/lib/utils/markdown';
import { Suite, assert, finish } from './support/harness';

const suite = new Suite('markdown-sanitize');

suite.check('does not pass a video element through', () => {
	const html = renderMarkdown('<video src=x onerror=alert(1)><script>alert(1)</script></video>');
	const lower = html.toLowerCase();
	assert(!lower.includes('<video'), html);
	assert(!lower.includes('<script'), html);
	assert(lower.includes('&lt;video'), html);
	assert(lower.includes('&lt;script'), html);
});

suite.check('escapes a raw image tag', () => {
	const html = renderMarkdown('<img src=x onerror=alert(1)>');
	assert(!html.includes('<img'), html);
	assert(html.includes('&lt;img'), html);
});

suite.check('still renders an allowed markdown link', () => {
	const html = renderMarkdown('[docs](https://example.com/docs)');
	assert(html.includes('<a href="https://example.com/docs"'), html);
	assert(html.includes('rel="noopener noreferrer"'), html);
});

finish(suite.result());
