/** Local Page intents work against the pinned CRDT, never a destroyed editor or current route. */
import type { LoroDocType } from 'loro-prosemirror';
import type { BodyInsertionBookmark } from './assets';
import { pageUndoManager } from '../editor-session';
let modules:
	| Promise<{
			pm: typeof import('prosemirror-model');
			state: typeof import('prosemirror-state');
			binding: typeof import('../vendor/loro-prosemirror');
			structure: typeof import('../vendor/loro-prosemirror/page-structure');
			schema: typeof import('../editor-schema');
	  }>
	| undefined;
function load() {
	return (modules ??= Promise.all([
		import('prosemirror-model'),
		import('prosemirror-state'),
		import('../vendor/loro-prosemirror'),
		import('../vendor/loro-prosemirror/page-structure'),
		import('../editor-schema')
	]).then(([pm, state, binding, structure, schema]) => ({
		pm,
		state,
		binding,
		structure,
		schema
	})));
}
export type BodyAtomIntent = {
	kind: 'image' | 'attachment' | 'childPage';
	attrs: Record<string, string>;
	replace?: boolean;
	merge?: boolean;
};
export async function applyBodyIntent(
	doc: LoroDocType,
	bookmark: BodyInsertionBookmark,
	intent: BodyAtomIntent,
	allowed: () => boolean
): Promise<{ blockId: string } | null> {
	const mods = await load();
	if (!allowed()) return null;
	const manager = await pageUndoManager(doc);
	if (!allowed()) return null;
	manager.addExcludeOriginPrefix('sys:storage');
	if (!allowed() || !mods.structure.hasPageStructure(doc)) return null;
	const entries = mods.structure.structureEntries(doc);
	const at = entries.findIndex((entry) => entry.block.id === bookmark.blockId);
	const schema = mods.schema.createPageSchema(mods.pm);
	const mapping = new Map();
	mods.structure.attachStructureContext(mapping, doc);
	if (bookmark.blockId === null) {
		if (entries.length || intent.replace || intent.merge || !allowed()) return null;
		const atom = schema.nodes[intent.kind].create({ ...intent.attrs, indent: 0 });
		const state = mods.state.EditorState.create({
			schema,
			doc: schema.topNodeType.createChecked(null, atom)
		});
		mods.binding.updateLoroToPmState(doc, mapping, state, undefined, manager, 'flow:doc-intent');
		const first = mods.structure.structureEntries(doc)[0];
		return first ? { blockId: first.block.id } : null;
	}
	if (at < 0) return null;
	const node = mods.binding.createNodeFromLoroObj(schema, doc.getMap('doc'), mapping);
	if (!node || Array.isArray(node)) return null;
	let state = mods.state.EditorState.create({ schema, doc: node });
	let offset = 0;
	for (let i = 0; i < at; i++) offset += state.doc.child(i).nodeSize;
	if (intent.merge && state.doc.child(at).type.name !== intent.kind) return null;
	const atom = schema.nodes[intent.kind].create({
		...(intent.merge ? state.doc.child(at).attrs : {}),
		...intent.attrs,
		indent: entries[at].depth
	});
	let inserted = atom;
	if (intent.replace) {
		if (state.doc.child(at).isTextblock && state.doc.child(at).content.size) return null;
		state = state.apply(state.tr.setNodeMarkup(offset, schema.nodes[intent.kind], atom.attrs));
		inserted = state.doc.nodeAt(offset)!;
	} else {
		let end = at;
		if (bookmark.side === 'after') {
			end = at + 1;
			while (end < entries.length && entries[end].depth > entries[at].depth) end++;
			offset = 0;
			for (let i = 0; i < end; i++) offset += state.doc.child(i).nodeSize;
		}
		state = state.apply(state.tr.insert(offset, atom));
	}
	if (!allowed()) return null;
	mods.binding.updateLoroToPmState(doc, mapping, state, undefined, manager, 'flow:doc-intent');
	const found = [...mapping].find(([, mapped]) => mapped === inserted);
	return found ? { blockId: found[0] } : null;
}

export async function bodyBlockPresent(doc: LoroDocType, blockId: string): Promise<boolean> {
	const mods = await load();
	return (
		mods.structure.hasPageStructure(doc) &&
		mods.structure.structureEntries(doc).some((entry) => entry.block.id === blockId)
	);
}
