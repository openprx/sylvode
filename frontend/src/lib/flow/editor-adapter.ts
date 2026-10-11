// `EditorAdapter`: ProseMirror bound to a single rich-text container via `loro-prosemirror`
// (ADR-0006: Svelte 5 + ProseMirror core + `loro-prosemirror`). Owns no Page tree, network, auth
// or business policy (`contracts/ui-surface-v1.md` "职责边界").
//
// v0.4 alpha block model (documented simplification, not hidden): Page content is a FLAT sequence
// of typed blocks, each carrying an `indent` attribute (0..7) instead of true structural nesting
// (bulletList > listItem > paragraph). This mirrors how Notion's own block model actually works
// (a flat list of blocks + an indent/parent reference, not nested-container prose) and keeps
// `nest`/`outdent` a single uniform command across every registered block type
// (`ui-surface-v1.md`'s "Renderer registry": paragraph/heading/list/code). No inline marks
// (bold/italic/etc.) in this round -- plain text per block only.
//
// ALL loro-crdt / loro-prosemirror / prosemirror-* imports are dynamic `import()` calls inside
// this module's functions, never static top-level imports. That is what keeps the engine chunk
// out of every bundle that does not reach the Flow route
// (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { EditorAdapterContract, EngineDiff, RelativeSelection } from './types';
import { isRegisteredBlockType } from './renderer-registry';
import {
	presenceInitials,
	safePresenceColor,
	type PresenceCursor,
	type RemotePresence
} from './presence';
// Type-only import: erased at compile time, so this does not pull `loro-prosemirror` into the
// module graph -- only its exported *type* `LoroDocType` (the specific `{doc, data}` container
// shape it expects) is used here, to type the doc reference the caller constructs at runtime via
// dynamic `import('loro-crdt')`.
import type { LoroDocType } from 'loro-prosemirror';

export const MAX_INDENT = 7;

type PmModule = typeof import('prosemirror-model');
type PmStateModule = typeof import('prosemirror-state');
type PmViewModule = typeof import('prosemirror-view');
type PmCommandsModule = typeof import('prosemirror-commands');
type PmKeymapModule = typeof import('prosemirror-keymap');
type LoroPmModule = typeof import('loro-prosemirror');

export type FlowBlockType = 'paragraph' | 'heading' | 'bulletItem' | 'codeBlock';

export interface SlashMenuState {
	readonly active: boolean;
	readonly query: string;
	readonly coords: { left: number; top: number } | null;
}

export interface EditorAdapterHooks {
	onSlashMenu?(state: SlashMenuState): void;
	onSelectionChange?(): void;
	/** The local caret/selection as a presence cursor (`null` when it is not inside a block). */
	onPresenceCursor?(cursor: PresenceCursor | null): void;
}

let modulesPromise: Promise<{
	pm: PmModule;
	pmState: PmStateModule;
	pmView: PmViewModule;
	pmCommands: PmCommandsModule;
	pmKeymap: PmKeymapModule;
	loroPm: LoroPmModule;
}> | null = null;

/** Loads the whole ProseMirror + Loro-binding engine bundle exactly once, lazily. */
function loadEngineModules() {
	if (!modulesPromise) {
		modulesPromise = Promise.all([
			import('prosemirror-model'),
			import('prosemirror-state'),
			import('prosemirror-view'),
			import('prosemirror-commands'),
			import('prosemirror-keymap'),
			import('loro-prosemirror')
		]).then(([pm, pmState, pmView, pmCommands, pmKeymap, loroPm]) => ({
			pm,
			pmState,
			pmView,
			pmCommands,
			pmKeymap,
			loroPm
		}));
	}
	return modulesPromise;
}

function buildSchema(pm: PmModule): InstanceType<PmModule['Schema']> {
	const indentAttr = { default: 0 };
	return new pm.Schema({
		nodes: {
			doc: { content: 'block+' },
			paragraph: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				toDOM: (node) => ['p', { 'data-indent': node.attrs.indent }, 0],
				parseDOM: [{ tag: 'p', getAttrs: (dom) => ({ indent: Number((dom as HTMLElement).dataset.indent ?? 0) }) }]
			},
			heading: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr, level: { default: 1 } },
				toDOM: (node) => [`h${node.attrs.level}`, { 'data-indent': node.attrs.indent }, 0],
				parseDOM: [1, 2, 3].map((level) => ({
					tag: `h${level}`,
					getAttrs: (dom) => ({ level, indent: Number((dom as HTMLElement).dataset.indent ?? 0) })
				}))
			},
			bulletItem: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				toDOM: (node) => ['div', { class: 'flow-bullet-item', 'data-indent': node.attrs.indent }, 0],
				parseDOM: [{ tag: 'div.flow-bullet-item' }]
			},
			codeBlock: {
				group: 'block',
				content: 'text*',
				attrs: { indent: indentAttr },
				marks: '',
				code: true,
				toDOM: (node) => ['pre', { 'data-indent': node.attrs.indent }, ['code', 0]],
				parseDOM: [{ tag: 'pre', preserveWhitespace: 'full' as const }]
			},
			text: { group: 'inline' }
		},
		marks: {}
	});
}

function emptyDoc(schema: InstanceType<PmModule['Schema']>) {
	return schema.node('doc', null, [schema.node('paragraph', { indent: 0 }, [])]);
}

export class FlowEditorAdapter implements EditorAdapterContract {
	private readonly doc: LoroDocType;
	private readonly hooks: EditorAdapterHooks;
	private view: import('prosemirror-view').EditorView | null = null;
	private schema: InstanceType<PmModule['Schema']> | null = null;
	private mods: Awaited<ReturnType<typeof loadEngineModules>> | null = null;
	private editable = true;
	private remotePeers: ReadonlyArray<RemotePresence> = [];
	private presenceKey: import('prosemirror-state').PluginKey | null = null;
	private lastCursorKey = '';

	constructor(doc: LoroDocType, hooks: EditorAdapterHooks = {}) {
		this.doc = doc;
		this.hooks = hooks;
	}

	/**
	 * Read-only mode (`view`/`comment` on the object, or write access revoked): the ProseMirror
	 * view stops accepting input, so no local transaction -- and therefore no engine update -- can
	 * be produced. Remote changes still render.
	 */
	setEditable(editable: boolean): void {
		this.editable = editable;
		this.view?.setProps({ editable: () => this.editable });
	}

	/**
	 * Renders remote peers' carets/selections and a block-level avatar. Decorations only: nothing
	 * here touches the document, so presence never enters the engine doc or the history, and it is
	 * never a lock -- the block stays editable for everyone.
	 */
	setRemotePresence(peers: ReadonlyArray<RemotePresence>): void {
		this.remotePeers = peers;
		if (!this.view || !this.presenceKey) return;
		this.view.dispatch(this.view.state.tr.setMeta(this.presenceKey, { peers }));
	}

	/** The position of the block whose Loro container id is `blockId`, or `null`. */
	private blockPosById(
		state: import('prosemirror-state').EditorState,
		blockId: string
	): { pos: number; size: number } | null {
		if (!this.mods) return null;
		const mapping = this.mods.loroPm.loroSyncPluginKey.getState(state)?.mapping;
		const node = mapping?.get(blockId as import('loro-crdt').ContainerID);
		if (!node || Array.isArray(node)) return null;
		let found: { pos: number; size: number } | null = null;
		state.doc.forEach((child, offset) => {
			if (!found && child === node) found = { pos: offset, size: child.content.size };
		});
		return found;
	}

	/** The local selection as a presence cursor: the block's Loro container id and in-block
	 * offsets. `null` when the selection spans blocks or the block is not mapped yet. */
	private presenceCursor(state: import('prosemirror-state').EditorState): PresenceCursor | null {
		if (!this.mods) return null;
		const { $anchor, $head } = state.selection;
		if ($anchor.depth < 1 || $head.depth < 1) return null;
		const block = $head.node(1);
		if ($anchor.node(1) !== block) {
			return this.cursorFor(state, block, $head.parentOffset, $head.parentOffset);
		}
		return this.cursorFor(state, block, $anchor.parentOffset, $head.parentOffset);
	}

	private cursorFor(
		state: import('prosemirror-state').EditorState,
		block: import('prosemirror-model').Node,
		anchor: number,
		head: number
	): PresenceCursor | null {
		const mapping = this.mods?.loroPm.loroSyncPluginKey.getState(state)?.mapping;
		if (!mapping) return null;
		for (const [id, mapped] of mapping) {
			if (mapped === block) return { blockId: id, anchor, head };
		}
		return null;
	}

	private reportPresence(state: import('prosemirror-state').EditorState): void {
		if (!this.hooks.onPresenceCursor || !this.view) return;
		const cursor = this.view.hasFocus() ? this.presenceCursor(state) : null;
		const key = cursor ? `${cursor.blockId}:${cursor.anchor}:${cursor.head}` : '';
		if (key === this.lastCursorKey) return;
		this.lastCursorKey = key;
		this.hooks.onPresenceCursor(cursor);
	}

	private buildPresencePlugin(
		pmState: PmStateModule,
		pmView: PmViewModule
	): import('prosemirror-state').Plugin {
		const key = new pmState.PluginKey('flowPresence');
		this.presenceKey = key;
		const decorate = (
			state: import('prosemirror-state').EditorState,
			peers: ReadonlyArray<RemotePresence>
		) => {
			const decorations: import('prosemirror-view').Decoration[] = [];
			for (const peer of peers) {
				if (!peer.cursor) continue;
				// Re-checked here so no peer-controlled string ever reaches a style, whatever
				// produced `peers` (`collab-protocol-v1.md`: colour only `#rrggbb`).
				const color = safePresenceColor(peer.user.color, peer.user.userId);
				const attribution: Record<string, string> = peer.attested
					? { 'data-presence-user': peer.user.userId }
					: {};
				const block = this.blockPosById(state, peer.cursor.blockId);
				if (!block) continue;
				const start = block.pos + 1;
				const clamp = (offset: number) => start + Math.min(offset, block.size);
				const anchor = clamp(peer.cursor.anchor);
				const head = clamp(peer.cursor.head);
				if (anchor !== head) {
					decorations.push(
						pmView.Decoration.inline(Math.min(anchor, head), Math.max(anchor, head), {
							class: 'flow-presence-selection',
							style: `background-color: ${color}33`,
							...attribution
						})
					);
				}
				decorations.push(
					pmView.Decoration.widget(
						head,
						() => {
							const caret = document.createElement('span');
							caret.className = 'flow-presence-caret';
							caret.setAttribute('data-testid', 'flow-presence-caret');
							if (peer.attested) caret.setAttribute('data-presence-user', peer.user.userId);
							// The name is rendered by CSS from this attribute, so it never becomes
							// part of the document's text (copy, find, text assertions). The peer
							// list next to the canvas carries the same names for screen readers.
							caret.setAttribute('data-presence-name', peer.user.name);
							caret.setAttribute('aria-hidden', 'true');
							caret.style.borderColor = color;
							caret.style.setProperty('--flow-presence-color', color);
							return caret;
						},
						{ key: `caret-${peer.sessionId}-${head}-${peer.user.name}`, side: 1 }
					),
					pmView.Decoration.widget(
						start,
						() => {
							const avatar = document.createElement('span');
							avatar.className = 'flow-presence-avatar';
							avatar.setAttribute('data-testid', 'flow-presence-avatar');
							avatar.setAttribute('aria-hidden', 'true');
							avatar.style.backgroundColor = color;
							avatar.title = peer.user.name;
							avatar.setAttribute('data-initials', presenceInitials(peer.user.name));
							return avatar;
						},
						{ key: `avatar-${peer.sessionId}-${block.pos}`, side: -1 }
					)
				);
			}
			return pmView.DecorationSet.create(state.doc, decorations);
		};
		return new pmState.Plugin({
			key,
			state: {
				init: (_config, state) => decorate(state, this.remotePeers),
				apply: (tr, previous, _old, newState) => {
					const meta = tr.getMeta(key) as { peers: ReadonlyArray<RemotePresence> } | undefined;
					if (meta) return decorate(newState, meta.peers);
					// Remote/local content changes move blocks: recompute from the same peers.
					return tr.docChanged ? decorate(newState, this.remotePeers) : previous.map(tr.mapping, tr.doc);
				}
			},
			props: {
				decorations: (state) => key.getState(state)
			}
		});
	}

	async mount(host: HTMLElement, _blockId: string): Promise<void> {
		const mods = await loadEngineModules();
		this.mods = mods;
		const { pm, pmState, pmView, pmCommands, pmKeymap, loroPm } = mods;
		const schema = buildSchema(pm);
		this.schema = schema;

		const nestOutdent = (delta: number): import('prosemirror-state').Command => (state, dispatch) => {
			const { $from } = state.selection;
			const depth = $from.parent.attrs.indent as number;
			const next = Math.max(0, Math.min(MAX_INDENT, depth + delta));
			if (next === depth) return false;
			if (dispatch) {
				const pos = $from.before($from.depth);
				dispatch(state.tr.setNodeAttribute(pos, 'indent', next));
			}
			return true;
		};

		const customKeymap = pmKeymap.keymap({
			'Mod-z': loroPm.undo,
			'Mod-y': loroPm.redo,
			'Mod-Shift-z': loroPm.redo,
			Tab: nestOutdent(1),
			'Shift-Tab': nestOutdent(-1)
		});
		const baseKeymapPlugin = pmKeymap.keymap(pmCommands.baseKeymap);

		const state = pmState.EditorState.create({
			schema,
			doc: emptyDoc(schema),
			plugins: [
				loroPm.LoroSyncPlugin({ doc: this.doc }),
				loroPm.LoroUndoPlugin({ doc: this.doc }),
				this.buildPresencePlugin(pmState, pmView),
				customKeymap,
				baseKeymapPlugin
			]
		});

		this.view = new pmView.EditorView(host, {
			state,
			editable: () => this.editable,
			dispatchTransaction: (tr) => {
				if (!this.view) return;
				// Focus fallback (`contracts/ui-surface-v1.md` a11y baseline: "focus 不因 remote
				// move/delete 丢到 body，使用最近合法 block fallback"): a remote import can delete
				// or move the block the user's selection/DOM focus was in. `loro-prosemirror`
				// already maps the existing selection through the resulting transaction to the
				// nearest still-valid position, but if the specific DOM node that held focus was
				// removed, the browser can drop focus to `document.body` even though the mapped
				// ProseMirror selection itself is fine. Re-focusing the view after such a
				// transaction lands focus back on that already-valid mapped selection instead of
				// leaving it stranded on body.
				const hadFocus = this.view.hasFocus();
				const newState = this.view.state.apply(tr);
				this.view.updateState(newState);
				if (hadFocus && !this.view.hasFocus() && typeof document !== 'undefined' && document.activeElement === document.body) {
					this.view.focus();
				}
				this.detectSlashMenu(newState);
				this.hooks.onSelectionChange?.();
				this.reportPresence(newState);
			}
		});
		host.addEventListener('focusout', () => {
			if (this.view) this.reportPresence(this.view.state);
		});
	}

	private detectSlashMenu(state: import('prosemirror-state').EditorState): void {
		if (!this.hooks.onSlashMenu || !this.view) return;
		// Read-only: no slash menu, nothing can be inserted.
		if (!this.editable) {
			this.hooks.onSlashMenu({ active: false, query: '', coords: null });
			return;
		}
		// IME guard (`contracts/ui-surface-v1.md` a11y/interaction baseline, task brief item 8):
		// while an IME composition is in progress (e.g. typing Chinese/Japanese pinyin/kana before
		// it resolves to committed characters), the intermediate, not-yet-committed text must never
		// be scanned for a "/" trigger -- doing so can pop the slash menu mid-composition on
		// perfectly ordinary text. `EditorView.composing` is ProseMirror's own tracking of the
		// native `compositionstart`/`compositionend` window, so no separate DOM listeners are
		// needed here.
		if (this.view.composing) return;
		const { $from } = state.selection;
		const textBefore = $from.parent.textBetween(0, $from.parentOffset, undefined, '￼');
		const match = /(?:^|\s)\/(\w*)$/.exec(textBefore);
		if (match) {
			const coords = this.view.coordsAtPos($from.pos);
			this.hooks.onSlashMenu({ active: true, query: match[1], coords: { left: coords.left, top: coords.bottom } });
		} else {
			this.hooks.onSlashMenu({ active: false, query: '', coords: null });
		}
	}

	/** Called by the slash menu UI when the user picks a block type; replaces the current block.
	 * Guarded by the renderer registry (`contracts/ui-surface-v1.md` "Renderer registry": "未注册
	 * type 失败为 read-only unsupported，不把 arbitrary component path 写入 CRDT") -- defense in
	 * depth alongside the caller's own `FlowBlockType`-typed union, since this is the one place a
	 * block type string reaches a CRDT-mutating transaction. */
	setBlockType(type: FlowBlockType, level?: number): void {
		if (!this.view || !this.schema || !isRegisteredBlockType(type)) return;
		const { state, dispatch } = this.view;
		const { $from } = state.selection;
		const pos = $from.before($from.depth);
		const nodeType = this.schema.nodes[type];
		const indent = $from.parent.attrs.indent as number;
		const attrs = type === 'heading' ? { indent, level: level ?? 1 } : { indent };
		const tr = state.tr.setNodeMarkup(pos, nodeType, attrs);
		// Strip the trailing "/query" the user typed to open the menu.
		const parentEnd = $from.after($from.depth) - 1;
		const parentStart = $from.before($from.depth) + 1;
		const text = state.doc.textBetween(parentStart, parentEnd, undefined, '￼');
		const slashIndex = text.lastIndexOf('/');
		if (slashIndex >= 0) {
			tr.delete(parentStart + slashIndex, parentEnd);
		}
		dispatch(tr);
		this.view.focus();
	}

	/** Removes the trailing "/query" that opened the slash menu without changing the block type.
	 * Used when the picked entry is not a text block (Collection, Forms reference/embed, convert),
	 * whose result is created by the server or the bridge rather than by this editor. */
	consumeSlashTrigger(): void {
		if (!this.view) return;
		const { state, dispatch } = this.view;
		const { $from } = state.selection;
		const parentEnd = $from.after($from.depth) - 1;
		const parentStart = $from.before($from.depth) + 1;
		const text = state.doc.textBetween(parentStart, parentEnd, undefined, '￼');
		const slashIndex = text.lastIndexOf('/');
		if (slashIndex < 0) return;
		dispatch(state.tr.delete(parentStart + slashIndex, parentEnd));
	}

	/** Backs the canvas's per-block hover handle nest/outdent buttons (task brief item 1's "block
	 * hover handle"), which act on whatever block the pointer is over rather than the current
	 * selection. Shares the same clamp-to-`MAX_INDENT` rule as the Tab/Shift-Tab keymap. */
	adjustIndentAtPos(pos: number, delta: number): void {
		if (!this.view) return;
		const { state } = this.view;
		const resolved = state.doc.resolve(Math.min(Math.max(pos, 0), state.doc.content.size));
		const depth = resolved.parent.attrs.indent as number;
		const next = Math.max(0, Math.min(MAX_INDENT, depth + delta));
		if (next === depth) return;
		const blockPos = resolved.before(resolved.depth);
		this.view.dispatch(state.tr.setNodeAttribute(blockPos, 'indent', next));
	}

	/** Maps a DOM point to a document position, for the hover handle to know which block the
	 * pointer is over. Returns `null` outside the editable area. */
	posAtClientPoint(x: number, y: number): number | null {
		if (!this.view) return null;
		return this.view.posAtCoords({ left: x, top: y })?.pos ?? null;
	}

	applyRemote(_change: EngineDiff): void {
		// `LoroSyncPlugin` subscribes to the bound Loro container itself and reconciles the
		// ProseMirror doc on every remote import automatically -- there is nothing this method
		// needs to do beyond existing for interface conformance
		// (`contracts/ui-surface-v1.md`'s `EditorAdapter.applyRemote`).
	}

	getSelection(): RelativeSelection | null {
		if (!this.view) return null;
		const { selection } = this.view.state;
		return { anchor: selection.anchor, head: selection.head };
	}

	restoreSelection(value: RelativeSelection): void {
		if (!this.view || !this.mods) return;
		const { pmState } = this.mods;
		const size = this.view.state.doc.content.size;
		const anchor = Math.min(value.anchor, size);
		const head = Math.min(value.head, size);
		const tr = this.view.state.tr.setSelection(pmState.TextSelection.create(this.view.state.doc, anchor, head));
		this.view.dispatch(tr);
	}

	undoLocal(): boolean {
		if (!this.view || !this.mods) return false;
		const { loroPm } = this.mods;
		if (!loroPm.canUndo(this.view.state)) return false;
		return loroPm.undo(this.view.state, this.view.dispatch);
	}

	redoLocal(): boolean {
		if (!this.view || !this.mods) return false;
		const { loroPm } = this.mods;
		if (!loroPm.canRedo(this.view.state)) return false;
		return loroPm.redo(this.view.state, this.view.dispatch);
	}

	async destroy(): Promise<void> {
		this.view?.destroy();
		this.view = null;
	}
}
