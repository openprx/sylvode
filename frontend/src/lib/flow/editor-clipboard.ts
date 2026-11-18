/** Plain Markdown is parsed into schema nodes, never executable HTML. HTML uses PM's schema parser. */
import type { Node, Schema } from 'prosemirror-model';
import { safeEditorHref } from './editor-schema';
/** Explicit plain paste bypasses Markdown, entity decoding, internal MIME and HTML. */
export function plainPasteBlocks(text: string, schema: Schema): Node[] {
	return text
		.replace(/\r\n?/g, '\n')
		.split('\n')
		.map((line) =>
			schema.nodes.paragraph.create({ indent: 0 }, line ? schema.text(line) : undefined)
		);
}

export function markdownBlocks(text: string, schema: Schema): Node[] | null {
	if (
		!/(?:&(?:amp|lt|gt|quot|apos|#\d+|#x[\da-f]+);)|(?:<\/?(?:strong|em|s|code|a)(?:\s|>))|(?:^|\n)(?:#{1,3} |[-*+] |\d+\. |\[ \] |> |```)|\*\*[^*]+\*\*|\[[^\]]+\]\([^)]+\)/.test(
			text
		)
	)
		return null;
	const blocks: Node[] = [];
	let code: string[] | null = null;
	let codeFenceLength = 3;
	for (const line of text.replace(/\r\n?/g, '\n').split('\n')) {
		const fence = /^(`{3,})[^`]*$/.exec(line);
		if (!code && fence) {
			code = [];
			codeFenceLength = fence[1].length;
			continue;
		}
		if (code && new RegExp('^`{' + codeFenceLength + ',}\\s*$').test(line)) {
			blocks.push(
				schema.nodes.codeBlock.create(
					{ indent: 0 },
					code.length ? schema.text(code.join('\n')) : undefined
				)
			);
			code = null;
			continue;
		}
		if (code) {
			code.push(line);
			continue;
		}
		let kind = 'paragraph',
			value = line;
		const attrs: Record<string, unknown> = { indent: 0 };
		let match: RegExpExecArray | null;
		if ((match = /^(#{1,3}) (.*)$/.exec(line))) {
			kind = 'heading';
			attrs.level = match[1].length;
			value = match[2];
		} else if ((match = /^[-*+] (.*)$/.exec(line))) {
			kind = 'bulletItem';
			value = match[1];
		} else if ((match = /^(\d+)\. (.*)$/.exec(line))) {
			kind = 'orderedItem';
			attrs.order = Number(match[1]);
			value = match[2];
		} else if ((match = /^\[([ xX])\] (.*)$/.exec(line))) {
			kind = 'todoItem';
			attrs.checked = match[1] !== ' ';
			value = match[2];
		} else if ((match = /^> (.*)$/.exec(line))) {
			kind = 'blockquote';
			value = match[1];
		}
		blocks.push(schema.nodes[kind].create(attrs, markdownInline(value, schema)));
	}
	if (code)
		blocks.push(
			schema.nodes.codeBlock.create(
				{ indent: 0 },
				code.length ? schema.text(code.join('\n')) : undefined
			)
		);
	return blocks;
}
function markdownInline(text: string, schema: Schema): Node[] {
	if (/<\/?(?:strong|em|s|code|a)(?:\s|>)/i.test(text)) return safeInlineHtml(text, schema);

	const nodes: Node[] = [];
	const pattern = /\*\*([^*]+)\*\*|~~([^~]+)~~|`([^`]+)`|\*([^*]+)\*|\[([^\]]+)\]\(([^)]+)\)/g;
	let offset = 0;
	for (const match of text.matchAll(pattern)) {
		if (match.index! > offset)
			nodes.push(schema.text(decodeEntities(text.slice(offset, match.index))));
		const kind = match[1]
			? 'bold'
			: match[2]
				? 'strike'
				: match[3]
					? 'code'
					: match[4]
						? 'italic'
						: 'link';
		const value = match[1] ?? match[2] ?? match[3] ?? match[4] ?? match[5];
		if (kind === 'link' && !safeEditorHref(decodeEntities(match[6])))
			nodes.push(schema.text(match[0]));
		else
			nodes.push(
				schema.text(kind === 'code' ? value : decodeEntities(value), [
					schema.marks[kind].create(
						kind === 'link' ? { href: decodeEntities(match[6]) } : undefined
					)
				])
			);
		offset = match.index! + match[0].length;
	}
	if (offset < text.length) nodes.push(schema.text(decodeEntities(text.slice(offset))));
	return nodes;
}

/** Recognizes only the exporter's balanced inline tags; every other tag remains literal text. */
function safeInlineHtml(text: string, schema: Schema): Node[] {
	const nodes: Node[] = [];
	const stack: { tag: string; mark: import('prosemirror-model').Mark }[] = [];
	const tags: Record<string, string> = {
		strong: 'bold',
		em: 'italic',
		s: 'strike',
		code: 'code',
		a: 'link'
	};
	for (const token of text.split(/(<[^>]*>)/g)) {
		if (!token) continue;
		const closing = /^<\/(strong|em|s|code|a)>$/i.exec(token);
		if (closing) {
			const index = stack.map((item) => item.tag).lastIndexOf(closing[1].toLowerCase());
			if (index >= 0) {
				stack.splice(index, 1);
				continue;
			}
		}
		const opening = /^<(strong|em|s|code)>$/i.exec(token);
		if (opening) {
			const tag = opening[1].toLowerCase();
			stack.push({ tag, mark: schema.marks[tags[tag]].create() });
			continue;
		}
		const link = /^<a href="([^"<>]*)">$/i.exec(token);
		if (link) {
			const href = decodeEntities(link[1]);
			if (safeEditorHref(href)) {
				stack.push({ tag: 'a', mark: schema.marks.link.create({ href }) });
				continue;
			}
		}
		nodes.push(
			schema.text(
				decodeEntities(token),
				stack.map((item) => item.mark)
			)
		);
	}
	return nodes;
}
function decodeEntities(text: string): string {
	return text.replace(/&(?:amp|lt|gt|quot|apos|#39|#\d+|#x[\da-f]+);/gi, (entity) => {
		const named: Record<string, string> = {
			'&amp;': '&',
			'&lt;': '<',
			'&gt;': '>',
			'&quot;': '"',
			'&apos;': "'",
			'&#39;': "'"
		};
		if (named[entity.toLowerCase()]) return named[entity.toLowerCase()];
		const hex = entity[2].toLowerCase() === 'x';
		const point = parseInt(entity.slice(hex ? 3 : 2, -1), hex ? 16 : 10);
		return point > 0 && point <= 0x10ffff ? String.fromCodePoint(point) : entity;
	});
}

export function pageClipboardNode(
	value: unknown,
	schema: Schema,
	allowBodyReferences = false
): Node {
	if (!value || typeof value !== 'object') throw new Error('Invalid clipboard block');
	const raw = value as { type?: unknown; attrs?: Record<string, unknown>; content?: unknown[] };
	if (typeof raw.type !== 'string' || !schema.nodes[raw.type] || ['doc', 'text'].includes(raw.type))
		throw new Error('Unsupported clipboard block');
	if (!allowBodyReferences && ['image', 'attachment', 'childPage'].includes(raw.type))
		throw new Error('Body references require an authorized copy or page picker.');
	const attrs = raw.attrs ?? {};
	const indent = attrs.indent ?? 0;
	if (!Number.isInteger(indent) || Number(indent) < 0 || Number(indent) > 7)
		throw new Error('Invalid clipboard depth');
	if (raw.type === 'heading' && ![1, 2, 3].includes(Number(attrs.level)))
		throw new Error('Invalid heading');
	if (raw.type === 'orderedItem' && (!Number.isSafeInteger(attrs.order) || Number(attrs.order) < 1))
		throw new Error('Invalid list number');
	if (raw.type === 'todoItem' && typeof attrs.checked !== 'boolean')
		throw new Error('Invalid checkbox');
	if (raw.type === 'toggle' && typeof attrs.collapsed !== 'boolean')
		throw new Error('Invalid toggle');
	for (const child of raw.content ?? []) {
		const text = child as {
			type?: unknown;
			text?: unknown;
			marks?: { type?: unknown; attrs?: { href?: unknown } }[];
		};
		if (text.type !== 'text' || typeof text.text !== 'string')
			throw new Error('Unsupported clipboard content');
		for (const mark of text.marks ?? []) {
			if (typeof mark.type !== 'string' || !schema.marks[mark.type])
				throw new Error('Unsupported clipboard mark');
			if (
				mark.type === 'link' &&
				(typeof mark.attrs?.href !== 'string' || !safeEditorHref(mark.attrs.href))
			)
				throw new Error('Unsafe clipboard link');
		}
	}
	const node = schema.nodeFromJSON(value);
	node.check();
	return node;
}
