/** Canonical Page editor schema. Mark names are native LoroText attribute keys. */
import type { Schema as SchemaType, NodeSpec, MarkSpec } from 'prosemirror-model';
export type FlowMark = 'bold' | 'italic' | 'strike' | 'code' | 'link';
export type FlowBlock =
	| 'paragraph'
	| 'heading'
	| 'bulletItem'
	| 'orderedItem'
	| 'todoItem'
	| 'blockquote'
	| 'toggle'
	| 'divider'
	| 'codeBlock'
	| 'image'
	| 'attachment'
	| 'childPage';
export function safeEditorHref(value: string): boolean {
	for (const char of value) {
		const code = char.codePointAt(0)!;
		if (code < 32 || code === 127) return false;
	}
	return (
		value.length > 0 &&
		value.length <= 2048 &&
		!/\s/.test(value) &&
		/^(https?:\/\/|mailto:|\/(?!\/)|#)/i.test(value)
	);
}
export function createPageSchema(pm: typeof import('prosemirror-model')): SchemaType {
	const indent = { default: 0 };
	const block = (tag: string, cls?: string, attrs: NodeSpec['attrs'] = {}): NodeSpec => ({
		group: 'block',
		content: 'text*',
		attrs: { indent, ...attrs },
		toDOM: (node) => [
			tag,
			{
				class: cls,
				'data-indent': node.attrs.indent,
				style: `margin-left:${node.attrs.indent * 1.5}rem`,
				...(node.type.name === 'orderedItem' ? { 'data-order': node.attrs.order } : {}),
				...(node.type.name === 'todoItem' ? { 'data-checked': String(node.attrs.checked) } : {}),
				...(node.type.name === 'toggle' ? { 'data-collapsed': String(node.attrs.collapsed) } : {})
			},
			0
		],
		parseDOM: [
			{
				tag: cls ? `${tag}.${cls}` : tag,
				getAttrs: (dom) => ({
					indent: Math.max(0, Math.min(7, Number((dom as HTMLElement).dataset.indent) || 0)),
					...(cls === 'flow-ordered-item'
						? { order: Number((dom as HTMLElement).dataset.order) || 1 }
						: {}),
					...(cls === 'flow-todo-item'
						? { checked: (dom as HTMLElement).dataset.checked === 'true' }
						: {})
				})
			}
		]
	});
	const simpleMark = (tag: string, extra: MarkSpec = {}): MarkSpec => ({
		inclusive: true,
		toDOM: () => [tag, 0],
		parseDOM: [{ tag }],
		...extra
	});
	return new pm.Schema({
		nodes: {
			doc: { content: 'block+' },
			paragraph: block('p'),
			heading: {
				...block('h1', undefined, { level: { default: 1 } }),
				toDOM: (node) => [
					`h${node.attrs.level}`,
					{ 'data-indent': node.attrs.indent, style: `margin-left:${node.attrs.indent * 1.5}rem` },
					0
				],
				parseDOM: [1, 2, 3].map((level) => ({
					tag: `h${level}`,
					getAttrs: () => ({ level, indent: 0 })
				}))
			},
			bulletItem: block('div', 'flow-bullet-item'),
			orderedItem: block('div', 'flow-ordered-item', { order: { default: 1 } }),
			todoItem: block('div', 'flow-todo-item', { checked: { default: false } }),
			blockquote: block('blockquote'),
			toggle: block('div', 'flow-toggle', { collapsed: { default: false } }),
			divider: {
				group: 'block',
				attrs: { indent },
				atom: true,
				toDOM: () => ['hr'],
				parseDOM: [{ tag: 'hr' }]
			},
			codeBlock: {
				...block('pre'),
				marks: '',
				code: true,
				toDOM: (node) => ['pre', { 'data-indent': node.attrs.indent }, ['code', 0]],
				parseDOM: [{ tag: 'pre', preserveWhitespace: 'full' }]
			},
			image: {
				group: 'block',
				atom: true,
				attrs: {
					indent,
					asset_id: { default: '' },
					alt: { default: '' },
					caption: { default: '' }
				},
				toDOM: (node) => [
					'figure',
					{ 'data-flow-block': 'image', 'data-indent': node.attrs.indent },
					['span', {}, node.attrs.alt || 'Image'],
					['figcaption', {}, node.attrs.caption]
				]
			},
			attachment: {
				group: 'block',
				atom: true,
				attrs: {
					indent,
					asset_id: { default: '' },
					name: { default: '' },
					mime: { default: '' },
					caption: { default: '' }
				},
				toDOM: (node) => [
					'div',
					{ 'data-flow-block': 'attachment', 'data-indent': node.attrs.indent },
					node.attrs.name || 'File'
				]
			},
			childPage: {
				group: 'block',
				atom: true,
				attrs: { indent, child_page_id: { default: '' } },
				toDOM: () => ['div', { 'data-flow-block': 'childPage' }, 'Linked page']
			},
			text: { group: 'inline' }
		},
		marks: {
			bold: simpleMark('strong', {
				parseDOM: [{ tag: 'strong' }, { tag: 'b' }, { style: 'font-weight=bold' }]
			}),
			italic: simpleMark('em', { parseDOM: [{ tag: 'em' }, { tag: 'i' }] }),
			strike: simpleMark('s', {
				parseDOM: [{ tag: 's' }, { tag: 'del' }, { style: 'text-decoration=line-through' }]
			}),
			code: simpleMark('code', { excludes: '_' }),
			link: {
				attrs: { href: {} },
				inclusive: false,
				toDOM: (mark) => [
					'a',
					{
						href: safeEditorHref(mark.attrs.href) ? mark.attrs.href : '#',
						rel: 'noopener noreferrer',
						target: '_blank'
					},
					0
				],
				parseDOM: [
					{
						tag: 'a[href]',
						getAttrs: (dom) => {
							const href = (dom as HTMLElement).getAttribute('href') ?? '';
							return safeEditorHref(href) ? { href } : false;
						}
					}
				]
			}
		}
	});
}
