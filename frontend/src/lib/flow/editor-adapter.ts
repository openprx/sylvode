// `EditorAdapter`: ProseMirror bound to a single rich-text container via `loro-prosemirror`
// (ADR-0006: Svelte 5 + ProseMirror core + `loro-prosemirror`). Owns no Page tree, network, auth
// or business policy (`contracts/ui-surface-v1.md` "职责边界").
//
// v0.4 alpha block model (documented simplification, not hidden): Page content is a FLAT sequence
// of typed blocks, each carrying an `indent` attribute (0..7) instead of true structural nesting
// (bulletList > listItem > paragraph). This mirrors how Notion's own block model actually works
// (a flat list of blocks + an indent/parent reference, not nested-container prose) and keeps
// `nest`/`outdent` a single uniform command across every registered block type
// (`ui-surface-v1.md`'s "Renderer registry": paragraph/heading/list/code). Inline marks are
// bold, italic, code, strike, and link. Markdown triggers, the block handle, and multi-block
// moves are transactions on this same document, so Loro collaboration and local undo see them.
// Triggers do not run while an IME composition is open.
//
// ALL loro-crdt / loro-prosemirror / prosemirror-* imports are dynamic `import()` calls inside
// this module's functions, never static top-level imports. That is what keeps the engine chunk
// out of every bundle that does not reach the Flow route
// (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { EditorAdapterContract, EngineDiff, RelativeSelection } from './types';
import { isRegisteredBlockType } from './renderer-registry';
import {
	FLOW_BLOCK_CLIPBOARD,
	acceptEditorInput,
	decodeBlocks,
	encodeBlocks,
	insertIndexAfterRemoval,
	matchBlockShortcut,
	matchInlineWrap,
	safeHref,
	type ClipboardBlock,
	type FlowBlockKind,
	type FlowMarkName
} from './editor-input';
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

export interface MarkFlags {
	readonly bold: boolean;
	readonly italic: boolean;
	readonly code: boolean;
	readonly strike: boolean;
	readonly link: boolean;
}

export interface SelectionToolbarState {
	readonly active: boolean;
	readonly coords: { left: number; top: number } | null;
	readonly marks: MarkFlags;
}

const NO_MARKS: MarkFlags = {
	bold: false,
	italic: false,
	code: false,
	strike: false,
	link: false
};

export interface EditorAdapterHooks {
	onSlashMenu?(state: SlashMenuState): void;
	onSelectionChange?(): void;
	onSelectionToolbar?(state: SelectionToolbarState): void;
	/** Mod-k, or the toolbar's link button, when the editor should ask for an address. */
	onLinkRequest?(): void;
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
				parseDOM: [
					{
						tag: 'p',
						getAttrs: (dom) => ({ indent: Number((dom as HTMLElement).dataset.indent ?? 0) })
					}
				]
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
				toDOM: (node) => [
					'div',
					{ class: 'flow-bullet-item', 'data-indent': node.attrs.indent },
					0
				],
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
		marks: {
			bold: {
				parseDOM: [
					{ tag: 'strong' },
					{
						tag: 'b',
						getAttrs: (dom) => ((dom as HTMLElement).style.fontWeight === 'normal' ? false : null)
					}
				],
				toDOM: () => ['strong', 0]
			},
			italic: {
				parseDOM: [{ tag: 'em' }, { tag: 'i', getAttrs: (dom) => ((dom as HTMLElement).style.fontStyle === 'normal' ? false : null) }],
				toDOM: () => ['em', 0]
			},
			code: {
				parseDOM: [{ tag: 'code' }],
				toDOM: () => ['code', 0],
				inclusive: false
			},
			strike: {
				parseDOM: [{ tag: 's' }, { tag: 'del' }, { tag: 'strike' }],
				toDOM: () => ['s', 0]
			},
			link: {
				attrs: { href: { default: null } },
				inclusive: false,
				parseDOM: [
					{
						tag: 'a[href]',
						getAttrs: (dom) => {
							const href = (dom as HTMLElement).getAttribute('href') ?? '';
							return safeHref(href) ? { href } : false;
						}
					}
				],
				toDOM: (mark) => [
					'a',
					{ href: mark.attrs.href as string, rel: 'noopener noreferrer' },
					0
				]
			}
		}
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
	private blockKey: import('prosemirror-state').PluginKey | null = null;
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
					return tr.docChanged
						? decorate(newState, this.remotePeers)
						: previous.map(tr.mapping, tr.doc);
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

		const nestOutdent =
			(delta: number): import('prosemirror-state').Command =>
			(state, dispatch) => {
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

		const toggle = (name: FlowMarkName) => {
			const mark = schema.marks[name];
			if (!mark) return () => false;
			const command = pmCommands.toggleMark(mark);
			return (
				state: import('prosemirror-state').EditorState,
				dispatch?: (tr: import('prosemirror-state').Transaction) => void
			) => {
				if (!this.view || !acceptEditorInput({ composing: this.view.composing, isComposing: false })) {
					return true;
				}
				return command(state, dispatch);
			};
		};
		const customKeymap = pmKeymap.keymap({
			'Mod-z': loroPm.undo,
			'Mod-y': loroPm.redo,
			'Mod-Shift-z': loroPm.redo,
			'Mod-b': toggle('bold'),
			'Mod-i': toggle('italic'),
			'Mod-e': toggle('code'),
			'Mod-Shift-s': toggle('strike'),
			'Mod-k': () => {
				if (!this.view || !acceptEditorInput({ composing: this.view.composing, isComposing: false })) {
					return true;
				}
				this.hooks.onLinkRequest?.();
				return true;
			},
			Tab: nestOutdent(1),
			'Shift-Tab': nestOutdent(-1),
			Backspace: (state, dispatch) => this.deleteSelectedBlocks(state, dispatch),
			Delete: (state, dispatch) => this.deleteSelectedBlocks(state, dispatch)
		});
		const baseKeymapPlugin = pmKeymap.keymap(pmCommands.baseKeymap);

		const state = pmState.EditorState.create({
			schema,
			doc: emptyDoc(schema),
			plugins: [
				loroPm.LoroSyncPlugin({ doc: this.doc }),
				loroPm.LoroUndoPlugin({ doc: this.doc }),
				this.buildPresencePlugin(pmState, pmView),
				this.buildEditingPlugin(pmState, pmView),
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
				if (
					hadFocus &&
					!this.view.hasFocus() &&
					typeof document !== 'undefined' &&
					document.activeElement === document.body
				) {
					this.view.focus();
				}
				this.detectSlashMenu(newState);
				this.hooks.onSelectionChange?.();
				this.reportPresence(newState);
				this.reportToolbar(newState);
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
			this.hooks.onSlashMenu({
				active: true,
				query: match[1],
				coords: { left: coords.left, top: coords.bottom }
			});
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
		const tr = this.view.state.tr.setSelection(
			pmState.TextSelection.create(this.view.state.doc, anchor, head)
		);
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

	toggleMark(name: FlowMarkName): void {
		if (!this.view || !this.mods || !this.schema) return;
		const mark = this.schema.marks[name];
		if (!mark) return;
		this.mods.pmCommands.toggleMark(mark)(this.view.state, this.view.dispatch);
		this.view.focus();
	}

	/** Adds a link to the selection, or removes it when `href` is null. An empty selection
	 * inserts the address as the linked text. Unsafe addresses are ignored. */
	setLink(href: string | null): void {
		if (!this.view || !this.schema) return;
		const mark = this.schema.marks.link;
		if (!mark) return;
		const { state } = this.view;
		const { from, to, empty } = state.selection;
		if (href === null) {
			if (!empty) this.view.dispatch(state.tr.removeMark(from, to, mark));
			this.view.focus();
			return;
		}
		if (!safeHref(href)) return;
		if (empty) {
			const node = this.schema.text(href, [mark.create({ href })]);
			this.view.dispatch(state.tr.replaceSelectionWith(node, false));
		} else {
			this.view.dispatch(state.tr.addMark(from, to, mark.create({ href })));
		}
		this.view.focus();
	}

	/** Index of the top-level block containing `pos`, or null outside the document. */
	blockIndexAt(pos: number): number | null {
		if (!this.view) return null;
		let found: number | null = null;
		let index = 0;
		const count = this.view.state.doc.childCount;
		this.view.state.doc.forEach((node, offset) => {
			const end = offset + node.nodeSize;
			if (found === null && pos >= offset && (pos < end || (index === count - 1 && pos === end))) {
				found = index;
			}
			index += 1;
		});
		return found;
	}

	blockCount(): number {
		return this.view?.state.doc.childCount ?? 0;
	}

	selectBlock(index: number, extend: boolean): void {
		if (!this.view || !this.blockKey) return;
		const count = this.view.state.doc.childCount;
		if (index < 0 || index >= count) return;
		const current = this.blockSelection();
		const anchor = extend && current ? current.anchor : index;
		this.view.dispatch(this.view.state.tr.setMeta(this.blockKey, { anchor, head: index }));
	}

	selectedRange(): { start: number; end: number } | null {
		const selection = this.blockSelection();
		if (!selection) return null;
		return {
			start: Math.min(selection.anchor, selection.head),
			end: Math.max(selection.anchor, selection.head) + 1
		};
	}

	moveBlockRange(start: number, end: number, dest: number): void {
		if (!this.view) return;
		const insertAt = insertIndexAfterRemoval(start, end, dest);
		if (insertAt === null) return;
		const blocks = this.topBlocks();
		const from = blocks[start]?.pos;
		const last = blocks[end - 1];
		const destBlock = blocks[dest];
		if (from === undefined || !last) return;
		const to = last.pos + last.node.nodeSize;
		const destPos = destBlock ? destBlock.pos : this.view.state.doc.content.size;
		const slice = this.view.state.doc.slice(from, to);
		const tr = this.view.state.tr.delete(from, to);
		tr.insert(tr.mapping.map(destPos), slice.content);
		if (this.blockKey) tr.setMeta(this.blockKey, null);
		this.view.dispatch(tr);
		this.view.focus();
	}

	deleteBlockRange(start: number, end: number): void {
		if (!this.view || !this.schema) return;
		const blocks = this.topBlocks();
		const from = blocks[start]?.pos;
		const last = blocks[end - 1];
		if (from === undefined || !last) return;
		const to = last.pos + last.node.nodeSize;
		const tr = this.view.state.tr;
		if (end - start >= this.view.state.doc.childCount) {
			tr.replaceWith(0, this.view.state.doc.content.size, this.schema.node('paragraph', { indent: 0 }));
		} else {
			tr.delete(from, to);
		}
		if (this.blockKey) tr.setMeta(this.blockKey, null);
		this.view.dispatch(tr);
		this.view.focus();
	}

	duplicateBlockRange(start: number, end: number): void {
		if (!this.view) return;
		const blocks = this.topBlocks();
		const from = blocks[start]?.pos;
		const last = blocks[end - 1];
		if (from === undefined || !last) return;
		const to = last.pos + last.node.nodeSize;
		const slice = this.view.state.doc.slice(from, to);
		this.view.dispatch(this.view.state.tr.insert(to, slice.content));
		this.view.focus();
	}

	setBlockTypeRange(start: number, end: number, type: FlowBlockKind, level?: number): void {
		if (!this.view || !this.schema || !isRegisteredBlockType(type)) return;
		const blocks = this.topBlocks();
		const tr = this.view.state.tr;
		for (let index = start; index < end; index += 1) {
			const block = blocks[index];
			if (!block) continue;
			const indent = block.node.attrs.indent as number;
			const attrs = type === 'heading' ? { indent, level: level ?? 1 } : { indent };
			tr.setNodeMarkup(block.pos, this.schema.nodes[type], attrs);
		}
		this.view.dispatch(tr);
		this.view.focus();
	}

	adjustIndentRange(start: number, end: number, delta: number): void {
		if (!this.view) return;
		const blocks = this.topBlocks();
		const tr = this.view.state.tr;
		for (let index = start; index < end; index += 1) {
			const block = blocks[index];
			if (!block) continue;
			const depth = block.node.attrs.indent as number;
			const next = Math.max(0, Math.min(MAX_INDENT, depth + delta));
			if (next !== depth) tr.setNodeAttribute(block.pos, 'indent', next);
		}
		this.view.dispatch(tr);
	}

	copySelectedBlocks(): string | null {
		const range = this.selectedRange();
		if (!range) return null;
		const blocks = this.clipboardBlocks(range.start, range.end);
		return blocks.length > 0 ? encodeBlocks(blocks) : null;
	}

	pasteBlocks(raw: string): boolean {
		const blocks = decodeBlocks(raw);
		if (!blocks || !this.view) return false;
		const range = this.selectedRange();
		const index = range ? range.end : (this.blockIndexAt(this.view.state.selection.from) ?? 0) + 1;
		return this.insertClipboard(blocks, index);
	}

	private insertClipboard(blocks: readonly ClipboardBlock[], index: number): boolean {
		if (!this.view || !this.schema) return false;
		const existing = this.topBlocks();
		const insertPos = existing[index]?.pos ?? this.view.state.doc.content.size;
		const tr = this.view.state.tr;
		let cursor = insertPos;
		const placed: Array<{ pos: number; block: ClipboardBlock }> = [];
		for (const block of blocks) {
			const node = this.bareBlock(block);
			if (!node) continue;
			tr.insert(cursor, node);
			placed.push({ pos: cursor, block });
			cursor += node.nodeSize;
		}
		for (const item of placed) {
			if (item.block.type === 'codeBlock') continue;
			const base = tr.mapping.map(item.pos) + 1;
			for (const mark of item.block.marks) {
				const type = this.schema.marks[mark.type];
				if (!type) continue;
				const created =
					mark.type === 'link' && mark.href ? type.create({ href: mark.href }) : type.create();
				tr.addMark(base + mark.start, base + mark.end, created);
			}
		}
		if (this.blockKey) tr.setMeta(this.blockKey, null);
		this.view.dispatch(tr);
		this.view.focus();
		return true;
	}

	private bareBlock(block: ClipboardBlock): import('prosemirror-model').Node | null {
		if (!this.schema) return null;
		const nodeType = this.schema.nodes[block.type];
		if (!nodeType) return null;
		const attrs =
			block.type === 'heading'
				? { indent: block.indent, level: block.level ?? 1 }
				: { indent: block.indent };
		const text = block.text.length > 0 ? this.schema.text(block.text) : null;
		return nodeType.create(attrs, text ? [text] : null);
	}

	private clipboardBlocks(start: number, end: number): ClipboardBlock[] {
		const blocks = this.topBlocks();
		const out: ClipboardBlock[] = [];
		for (let index = start; index < end; index += 1) {
			const block = blocks[index];
			if (!block) continue;
			const type = block.node.type.name;
			if (!isRegisteredBlockType(type)) continue;
			const marks: ClipboardBlock['marks'][number][] = [];
			if (type !== 'codeBlock') {
				block.node.forEach((child, offset) => {
					if (!child.isText || !child.text) return;
					for (const mark of child.marks) {
						const name = mark.type.name;
						if (name !== 'bold' && name !== 'italic' && name !== 'code' && name !== 'strike' && name !== 'link')
							continue;
						const href = typeof mark.attrs.href === 'string' ? mark.attrs.href : undefined;
						marks.push({
							start: offset,
							end: offset + child.text.length,
							type: name,
							...(href ? { href } : {})
						});
					}
				});
			}
			out.push({
				type,
				indent: (block.node.attrs.indent as number) ?? 0,
				...(type === 'heading' ? { level: (block.node.attrs.level as 1 | 2 | 3) ?? 1 } : {}),
				text: block.node.textContent,
				marks
			});
		}
		return out;
	}

	private topBlocks(): Array<{
		pos: number;
		node: import('prosemirror-model').Node;
	}> {
		const blocks: Array<{ pos: number; node: import('prosemirror-model').Node }> = [];
		this.view?.state.doc.forEach((node, offset) => {
			blocks.push({ pos: offset, node });
		});
		return blocks;
	}

	private blockSelection(): { anchor: number; head: number } | null {
		if (!this.view || !this.blockKey) return null;
		return this.blockKey.getState(this.view.state) as { anchor: number; head: number } | null;
	}

	private deleteSelectedBlocks(
		_state: import('prosemirror-state').EditorState,
		dispatch?: (tr: import('prosemirror-state').Transaction) => void
	): boolean {
		if (!this.view || !acceptEditorInput({ composing: this.view.composing, isComposing: false })) {
			return false;
		}
		const range = this.selectedRange();
		if (!range) return false;
		if (dispatch) this.deleteBlockRange(range.start, range.end);
		return true;
	}

	private reportToolbar(state: import('prosemirror-state').EditorState): void {
		if (!this.hooks.onSelectionToolbar || !this.view) return;
		if (!this.editable || this.view.composing || state.selection.empty) {
			this.hooks.onSelectionToolbar({ active: false, coords: null, marks: NO_MARKS });
			return;
		}
		const coords = this.view.coordsAtPos(state.selection.from);
		this.hooks.onSelectionToolbar({
			active: true,
			coords: { left: coords.left, top: coords.top },
			marks: this.markFlags(state)
		});
	}

	private markFlags(state: import('prosemirror-state').EditorState): MarkFlags {
		if (!this.schema) return NO_MARKS;
		const { from, to, empty, $from } = state.selection;
		const active = (name: FlowMarkName) => {
			const mark = this.schema?.marks[name];
			if (!mark) return false;
			return empty ? !!mark.isInSet($from.marks()) : state.doc.rangeHasMark(from, to, mark);
		};
		return {
			bold: active('bold'),
			italic: active('italic'),
			code: active('code'),
			strike: active('strike'),
			link: active('link')
		};
	}

	private buildEditingPlugin(
		pmState: PmStateModule,
		pmView: PmViewModule
	): import('prosemirror-state').Plugin {
		const key = new pmState.PluginKey('flowBlocks');
		this.blockKey = key;
		const adapter = this;
		return new pmState.Plugin({
			key,
			state: {
				init: () => null as { anchor: number; head: number } | null,
				apply: (tr, previous) => {
					const meta = tr.getMeta(key) as { anchor: number; head: number } | null | undefined;
					if (meta !== undefined) return meta;
					if (tr.docChanged || tr.selectionSet) return null;
					return previous;
				}
			},
			props: {
				decorations: (state) => {
					const selection = key.getState(state) as { anchor: number; head: number } | null;
					if (!selection) return pmView.DecorationSet.empty;
					const start = Math.min(selection.anchor, selection.head);
					const end = Math.max(selection.anchor, selection.head);
					const decorations: import('prosemirror-view').Decoration[] = [];
					let index = 0;
					state.doc.forEach((node, offset) => {
						if (index >= start && index <= end) {
							decorations.push(
								pmView.Decoration.node(offset, offset + node.nodeSize, {
									class: 'flow-block-selected'
								})
							);
						}
						index += 1;
					});
					return pmView.DecorationSet.create(state.doc, decorations);
				},
				handleTextInput: (view, from, to, text) => adapter.applyTextInput(view, from, to, text),
				handlePaste: (_view, event) => {
					const raw = event.clipboardData?.getData(FLOW_BLOCK_CLIPBOARD) ?? '';
					if (!raw) return false;
					if (adapter.pasteBlocks(raw)) {
						event.preventDefault();
						return true;
					}
					return false;
				},
				handleDOMEvents: {
					copy: (view, event) => adapter.copyEvent(view, event, false),
					cut: (view, event) => adapter.copyEvent(view, event, true)
				}
			}
		});
	}

	private copyEvent(
		_view: import('prosemirror-view').EditorView,
		event: Event,
		cut: boolean
	): boolean {
		const clipboard = event as ClipboardEvent;
		const range = this.selectedRange();
		if (!range || !clipboard.clipboardData) return false;
		const blocks = this.clipboardBlocks(range.start, range.end);
		if (blocks.length === 0) return false;
		clipboard.clipboardData.setData(FLOW_BLOCK_CLIPBOARD, encodeBlocks(blocks));
		clipboard.clipboardData.setData('text/plain', blocks.map((block) => block.text).join('\n'));
		clipboard.preventDefault();
		if (cut) this.deleteBlockRange(range.start, range.end);
		return true;
	}

	private applyTextInput(
		view: import('prosemirror-view').EditorView,
		from: number,
		to: number,
		text: string
	): boolean {
		if (!this.schema || from !== to) return false;
		if (!acceptEditorInput({ composing: view.composing, isComposing: false })) return false;
		const $from = view.state.doc.resolve(from);
		if ($from.parent.type.name === 'codeBlock') return false;
		const before = $from.parent.textBetween(0, $from.parentOffset, undefined, '\ufffc');
		const after = $from.parent.textBetween($from.parentOffset, $from.parent.content.size, undefined, '\ufffc');
		const block = matchBlockShortcut(before, text, after);
		if (block && isRegisteredBlockType(block.type)) {
			const start = $from.start();
			const blockPos = $from.before($from.depth);
			const tr = view.state.tr;
			if (block.deleteBefore > 0) tr.delete(start + before.length - block.deleteBefore, from);
			const insertAt = tr.mapping.map(start + before.length - block.deleteBefore);
			if (block.remainder) tr.insertText(block.remainder, insertAt);
			const indent = $from.parent.attrs.indent as number;
			const attrs = block.type === 'heading' ? { indent, level: block.level ?? 1 } : { indent };
			tr.setNodeMarkup(tr.mapping.map(blockPos), this.schema.nodes[block.type], attrs);
			view.dispatch(tr);
			return true;
		}
		const wrap = matchInlineWrap(before, text);
		if (!wrap) return false;
		const mark = this.schema.marks[wrap.mark];
		if (!mark) return false;
		// Edit the existing text container. Replacing the block would drop concurrent edits and
		// the marks already on the surrounding text.
		const blockStart = $from.start();
		const tr = view.state.tr;
		if (wrap.deleteTail > 0) tr.delete(from - wrap.deleteTail, from);
		const openFrom = tr.mapping.map(blockStart + wrap.openStart);
		tr.delete(openFrom, openFrom + wrap.openLength);
		const contentLen = wrap.contentEnd - wrap.contentStart;
		tr.addMark(openFrom, openFrom + contentLen, mark.create());
		view.dispatch(tr);
		return true;
	}

	async destroy(): Promise<void> {
		this.view?.destroy();
		this.view = null;
	}
}
