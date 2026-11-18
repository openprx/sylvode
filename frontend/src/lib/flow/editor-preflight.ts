/** Pure validation must finish before any native mutation, commit, fork or undo grouping. */
import type { Node } from 'prosemirror-model';
import { safeEditorHref, type FlowBlock } from './editor-schema';
import { EditorPreflightError } from './editor-batch';
const kinds: ReadonlySet<string> = new Set<FlowBlock>([
	'paragraph',
	'heading',
	'bulletItem',
	'orderedItem',
	'todoItem',
	'blockquote',
	'toggle',
	'divider',
	'codeBlock',
	'image',
	'attachment',
	'childPage'
]);
export function preflightEditorDocument(node: Node, indexed: boolean): void {
	const invalid = (reason: string): never => {
		throw new EditorPreflightError(reason);
	};
	if (node.type.name !== 'doc' || !node.childCount) invalid('Unsupported editor document.');
	try {
		node.check();
	} catch {
		invalid('Unsupported or invalid editor content.');
	}
	let previous = -1;
	node.forEach((block) => {
		if (!kinds.has(block.type.name)) invalid('Unsupported editor block.');
		const depth = block.attrs.indent;
		if (!Number.isInteger(depth) || depth < 0 || depth > 7) invalid('Invalid block depth.');
		if (indexed && depth > previous + 1) invalid('Block nesting skips a parent.');
		previous = depth;
		if (block.type.name === 'heading' && ![1, 2, 3].includes(block.attrs.level))
			invalid('Invalid heading level.');
		if (
			block.type.name === 'orderedItem' &&
			(!Number.isInteger(block.attrs.order) ||
				block.attrs.order < 1 ||
				block.attrs.order > 1_000_000)
		)
			invalid('Invalid list number.');
		if (block.type.name === 'todoItem' && typeof block.attrs.checked !== 'boolean')
			invalid('Invalid checkbox state.');
		if (block.type.name === 'toggle' && typeof block.attrs.collapsed !== 'boolean')
			invalid('Invalid toggle state.');
		const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
		if (
			['image', 'attachment'].includes(block.type.name) &&
			!uuid.test(String(block.attrs.asset_id ?? ''))
		)
			invalid('Upload or copy a file before inserting it.');
		if (block.type.name === 'childPage' && !uuid.test(String(block.attrs.child_page_id ?? '')))
			invalid('Choose an authorized page first.');
		for (const key of ['alt', 'caption', 'name', 'mime'])
			if (
				key in block.attrs &&
				(typeof block.attrs[key] !== 'string' || block.attrs[key].length > 4096)
			)
				invalid('Invalid block details.');
		block.forEach((text) => {
			if (!text.isText) invalid('Unsupported inline content.');
			for (const char of text.text ?? '') {
				if (char.length === 1 && char.charCodeAt(0) >= 0xd800 && char.charCodeAt(0) <= 0xdfff)
					invalid('Invalid Unicode text.');
			}
			for (const mark of text.marks) {
				if (!['bold', 'italic', 'strike', 'code', 'link'].includes(mark.type.name))
					invalid('Unsupported inline formatting.');
				if (
					mark.type.name === 'link' &&
					(typeof mark.attrs.href !== 'string' || !safeEditorHref(mark.attrs.href))
				)
					invalid('Unsafe link address.');
			}
		});
	});
}
