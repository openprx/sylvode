/**
 * Readable plain text from a `rich_text` value (stored as HTML).
 *
 * Record content is never injected as HTML: there is no sanitiser in this app, and `rich_text` is
 * shared between workspace members. Instead the HTML is parsed into an inert document
 * (`DOMParser`, which neither runs scripts nor loads resources) and its text nodes are walked:
 * block ends and `<br>` become line breaks, list items become "• " lines, `<script>`/`<style>` and
 * similar content is dropped, and entities come out decoded because the parser decodes them. The
 * result is rendered as text with `white-space: pre-line`.
 *
 * Where `DOMParser` is unavailable (unit tests, non-browser callers) a small tokenizer builds the
 * same node shape, so both paths share one walker.
 */

export interface TextTreeNode {
	nodeType: number;
	nodeName: string;
	nodeValue: string | null;
	childNodes: ArrayLike<TextTreeNode>;
}

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;

const DROPPED = new Set([
	'SCRIPT',
	'STYLE',
	'TEMPLATE',
	'NOSCRIPT',
	'IFRAME',
	'OBJECT',
	'EMBED',
	'HEAD',
	'TITLE',
	'SVG',
	'MATH',
	'CANVAS',
	'SELECT',
	'TEXTAREA',
	'BUTTON'
]);

const BLOCKS = new Set([
	'P',
	'DIV',
	'UL',
	'OL',
	'LI',
	'H1',
	'H2',
	'H3',
	'H4',
	'H5',
	'H6',
	'BLOCKQUOTE',
	'PRE',
	'SECTION',
	'ARTICLE',
	'HEADER',
	'FOOTER',
	'ASIDE',
	'NAV',
	'TABLE',
	'THEAD',
	'TBODY',
	'TFOOT',
	'TR',
	'DL',
	'DT',
	'DD',
	'FIGURE',
	'FIGCAPTION',
	'ADDRESS',
	'HR',
	'BODY',
	'HTML'
]);

const LINE_BREAK = '\u0000';

function walk(node: TextTreeNode, out: string[], preformatted: boolean): void {
	if (node.nodeType === TEXT_NODE) {
		const text = node.nodeValue ?? '';
		out.push(preformatted ? text.replace(/\r?\n/g, LINE_BREAK) : text.replace(/\s+/g, ' '));
		return;
	}
	if (node.nodeType !== ELEMENT_NODE && node.nodeType !== 9 && node.nodeType !== 11) return;
	const name = node.nodeName.toUpperCase();
	if (DROPPED.has(name)) return;
	if (name === 'BR') {
		out.push(LINE_BREAK);
		return;
	}
	const block = BLOCKS.has(name);
	if (block) out.push(LINE_BREAK);
	if (name === 'LI') out.push('• ');
	if (name === 'TD' || name === 'TH') out.push(' ');
	const children = node.childNodes;
	for (let index = 0; index < children.length; index += 1) {
		walk(children[index], out, preformatted || name === 'PRE');
	}
	if (block) out.push(LINE_BREAK);
}

/** Joins walked chunks into text: spaces collapsed around line breaks, at most one blank line. */
function finish(chunks: string[]): string {
	return chunks
		.join('')
		.split(LINE_BREAK)
		.map((line) => line.replace(/[ \t\u00a0]+/g, ' ').trim())
		.join('\n')
		.replace(/\n{3,}/g, '\n\n')
		.trim();
}

export function textFromTree(root: TextTreeNode): string {
	const chunks: string[] = [];
	walk(root, chunks, false);
	return finish(chunks);
}

// Fallback tokenizer.

const NAMED_ENTITIES: Record<string, string> = {
	amp: '&',
	lt: '<',
	gt: '>',
	quot: '"',
	apos: "'",
	nbsp: '\u00a0',
	copy: '©',
	reg: '®',
	hellip: '…',
	mdash: '—',
	ndash: '–',
	lsquo: '‘',
	rsquo: '’',
	ldquo: '“',
	rdquo: '”',
	bull: '•',
	middot: '·'
};

export function decodeEntities(text: string): string {
	return text.replace(/&(#x[0-9a-f]+|#[0-9]+|[a-z]+);/gi, (match, body: string) => {
		if (body[0] === '#') {
			const code =
				body[1] === 'x' || body[1] === 'X'
					? parseInt(body.slice(2), 16)
					: parseInt(body.slice(1), 10);
			return Number.isFinite(code) && code > 0 && code <= 0x10ffff
				? String.fromCodePoint(code)
				: match;
		}
		return NAMED_ENTITIES[body.toLowerCase()] ?? match;
	});
}

interface MutableNode extends TextTreeNode {
	childNodes: MutableNode[];
	parent: MutableNode | null;
}

const VOID = new Set([
	'BR',
	'HR',
	'IMG',
	'INPUT',
	'META',
	'LINK',
	'AREA',
	'BASE',
	'COL',
	'WBR',
	'SOURCE',
	'TRACK',
	'EMBED'
]);
const RAW_TEXT = new Set([
	'SCRIPT',
	'STYLE',
	'TEXTAREA',
	'TITLE',
	'NOSCRIPT',
	'IFRAME',
	'TEMPLATE'
]);

function element(name: string, parent: MutableNode | null): MutableNode {
	return { nodeType: ELEMENT_NODE, nodeName: name, nodeValue: null, childNodes: [], parent };
}

/** A forgiving HTML tokenizer: enough structure for {@link textFromTree}, nothing more. */
export function parseHtmlToTree(html: string): TextTreeNode {
	const root = element('BODY', null);
	let current = root;
	let index = 0;
	const pushText = (text: string) => {
		if (text)
			current.childNodes.push({
				nodeType: TEXT_NODE,
				nodeName: '#text',
				nodeValue: decodeEntities(text),
				childNodes: [],
				parent: current
			});
	};
	while (index < html.length) {
		const open = html.indexOf('<', index);
		if (open === -1) {
			pushText(html.slice(index));
			break;
		}
		pushText(html.slice(index, open));
		if (html.startsWith('<!--', open)) {
			const end = html.indexOf('-->', open + 4);
			index = end === -1 ? html.length : end + 3;
			continue;
		}
		const match = /^<\/?([a-zA-Z][a-zA-Z0-9-]*)[^>]*>/.exec(html.slice(open));
		if (!match) {
			// A stray "<" that does not start a tag is text.
			pushText('<');
			index = open + 1;
			continue;
		}
		const name = match[1].toUpperCase();
		index = open + match[0].length;
		if (match[0][1] === '/') {
			let cursor: MutableNode | null = current;
			while (cursor && cursor.nodeName !== name) cursor = cursor.parent;
			if (cursor && cursor.parent) current = cursor.parent;
			continue;
		}
		const node = element(name, current);
		current.childNodes.push(node);
		if (RAW_TEXT.has(name)) {
			const close = html.toLowerCase().indexOf(`</${name.toLowerCase()}`, index);
			const end = close === -1 ? html.length : close;
			node.childNodes.push({
				nodeType: TEXT_NODE,
				nodeName: '#text',
				nodeValue: html.slice(index, end),
				childNodes: [],
				parent: node
			});
			const closeEnd = close === -1 ? html.length : html.indexOf('>', close);
			index = closeEnd === -1 ? html.length : closeEnd + 1;
			continue;
		}
		if (!VOID.has(name) && !match[0].endsWith('/>')) current = node;
	}
	return root;
}

type HtmlParser = (html: string) => TextTreeNode | null;

function domParser(html: string): TextTreeNode | null {
	if (typeof DOMParser === 'undefined') return null;
	try {
		const doc = new DOMParser().parseFromString(html, 'text/html');
		return doc.body as unknown as TextTreeNode;
	} catch {
		return null;
	}
}

/** Plain text for a `rich_text` value. Never returns markup meant to be interpreted. */
export function richTextToPlainText(html: unknown, parse: HtmlParser = domParser): string {
	if (typeof html !== 'string' || !html.trim()) return '';
	const tree = parse(html) ?? parseHtmlToTree(html);
	return textFromTree(tree);
}
