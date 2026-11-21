import { issueBodyClipboard, trustedBodyClipboard } from './body/clipboard';
import { writable } from 'svelte/store';
import type { PageBodyJobs } from './body/jobs';
import { EditorWriteFailure, atomicEditorWrite } from './editor-batch';
import { pageTextMatches, visiblePageBlocks, type PageTextMatch } from './editor-page-tools';
// `EditorAdapter`: ProseMirror bound to a single rich-text container via `loro-prosemirror`
// (ADR-0006: Svelte 5 + ProseMirror core + `loro-prosemirror`). Owns no Page tree, network, auth
// or business policy (`contracts/ui-surface-v1.md` "职责边界").
//
// Canonical Page text blocks use native LoroText marks and explicit indent subtree semantics.
// Stable block movement is a separate identity-preserving binding contract.
//
// ALL loro-crdt / loro-prosemirror / prosemirror-* imports are dynamic `import()` calls inside
// this module's functions, never static top-level imports. That is what keeps the engine chunk
// out of every bundle that does not reach the Flow route
// (`contracts/ui-surface-v1.md` "engine chunk 只从 `(app)/flow` 动态 import").

import type { EditorAdapterContract, EngineDiff, RelativeSelection } from './types';
import { markdownBlocks, pageClipboardNode, plainPasteBlocks } from './editor-clipboard';
import { pageUndoManager, editorSessionDiagnostic, editorSessionAvailable } from './editor-session';
import { createPageSchema, safeEditorHref, type FlowMark, type FlowBlock } from './editor-schema';
import { isRegisteredBlockType } from './renderer-registry';
import {
	presenceInitials,
	presenceInk,
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
type LoroPmModule = typeof import('./vendor/loro-prosemirror/index');

export type FlowBlockType = FlowBlock;

export interface SlashMenuState {
	readonly active: boolean;
	readonly query: string;
	readonly coords: { left: number; top: number } | null;
}

export interface EditorAdapterHooks {
	onSlashMenu?(state: SlashMenuState): void;
	onSelectionChange?(): void;
	onDocumentChange?(): void;
	onKeyDown?(event: KeyboardEvent): boolean;
	onInputError?(message: string): void;
	onClipboardError?(reason: 'cut' | 'paste'): void;
	onPasteModeChange?(plain: boolean): void;
	body?: { objectId: string; jobs: PageBodyJobs | null };
	onBodyAction?(
		mode: 'replace' | 'details',
		blockId: string,
		kind: 'image' | 'attachment',
		attrs: Record<string, unknown>
	): void;
	ensureStructure?(): Promise<boolean>;
	lifecycleInfo?(): { refs: number; generation: number; retired: number; paused: boolean } | null;
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
	bodyViews: typeof import('./body/node-views');
	blockCut: typeof import('./editor-block-cut');
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
			import('./vendor/loro-prosemirror/index'),
			import('./body/node-views'),
			import('./editor-block-cut')
		]).then(([pm, pmState, pmView, pmCommands, pmKeymap, loroPm, bodyViews, blockCut]) => ({
			pm,
			pmState,
			pmView,
			pmCommands,
			pmKeymap,
			loroPm,
			bodyViews,
			blockCut
		}));
	}
	return modulesPromise;
}

/** Logical anchors resolve only through the explicit CRDT alias map. */
export function resolveEditorBlockId(doc: LoroDocType, blockId: string): string {
	const alias = (doc as import('loro-crdt').LoroDoc).getMap('flow_editor_aliases').get(blockId);
	return typeof alias === 'string' ? alias : blockId;
}

export function buildSchema(pm: PmModule): InstanceType<PmModule['Schema']> {
	return createPageSchema(pm);
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
	private bodyEditable = writable(true);
	private remotePeers: ReadonlyArray<RemotePresence> = [];
	private presenceKey: import('prosemirror-state').PluginKey | null = null;
	private lastCursorKey = '';
	private dismissedSlash = '';
	private destroyed = false;
	private findQuery = '';
	private findDoc: import('prosemirror-model').Node | undefined;
	private findMatches: PageTextMatch[] = [];
	private findIndex = -1;
	private plainPasteUntil = 0;
	private plainPasteArmed = false;
	private copiedBlockRange: { from: number; to: number } | null = null;

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
		this.bodyEditable.set(editable);
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

	/** The Loro container id of the block containing `pos`, for a comment anchored to that block. */
	blockIdAtPos(pos: number): string | null {
		if (!this.view) return null;
		const state = this.view.state;
		const clamped = Math.max(0, Math.min(pos, state.doc.content.size));
		const $pos = state.doc.resolve(clamped);
		if ($pos.depth < 1) {
			const node = state.doc.childAfter(clamped).node ?? state.doc.childBefore(clamped).node;
			return node ? (this.cursorFor(state, node, 0, 0)?.blockId ?? null) : null;
		}
		return this.cursorFor(state, $pos.node(1), 0, 0)?.blockId ?? null;
	}

	/** The position of the block whose Loro container id is `blockId`, or `null`. */
	private blockPosById(
		state: import('prosemirror-state').EditorState,
		blockId: string
	): { pos: number; size: number } | null {
		if (!this.mods) return null;
		const mapping = this.mods.loroPm.loroSyncPluginKey.getState(state)?.mapping;
		const node = mapping?.get(
			resolveEditorBlockId(this.doc, blockId) as import('loro-crdt').ContainerID
		);
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
							caret.style.setProperty('--flow-presence-ink', presenceInk(color));
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
							avatar.style.color = presenceInk(color);
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

	/** Native selectionchange can lag a clipboard/mouse event. Reconcile before a writing command,
	 * using the public view position API, so a visible caret can never edit a previous block. */
	private syncSelectionFromDOM(view: import('prosemirror-view').EditorView): void {
		if (!this.mods || view.composing || !view.hasFocus()) return;
		const selection = view.dom.ownerDocument.getSelection();
		if (
			!selection?.anchorNode ||
			!selection.focusNode ||
			!view.dom.contains(selection.anchorNode) ||
			!view.dom.contains(selection.focusNode)
		)
			return;
		try {
			const anchor = view.posAtDOM(selection.anchorNode, selection.anchorOffset, 1);
			const head = selection.isCollapsed
				? anchor
				: view.posAtDOM(selection.focusNode, selection.focusOffset, -1);
			if (anchor === view.state.selection.anchor && head === view.state.selection.head) return;
			const next = this.mods.pmState.TextSelection.between(
				view.state.doc.resolve(anchor),
				view.state.doc.resolve(head)
			);
			view.dispatch(view.state.tr.setSelection(next));
		} catch {
			/* A remote DOM update can detach a native endpoint; retain PM's valid mapped selection. */
		}
	}

	private buildWritingPlugin(
		pmState: PmStateModule,
		pmView: PmViewModule
	): import('prosemirror-state').Plugin {
		return new pmState.Plugin({
			props: {
				handleTextInput: (view, from, to, text) => {
					if (
						view.composing ||
						!this.editable ||
						from !== to ||
						!view.state.selection.$from.parent.isTextblock
					)
						return false;
					const { $from } = view.state.selection;
					if ($from.parent.type.name === 'codeBlock') return false;
					const before = $from.parent.textBetween(0, $from.parentOffset);
					const combined = before + text;
					const patterns: [RegExp, FlowBlockType, Record<string, unknown>][] = [
						[/^(#{1,3}) $/, 'heading', { level: combined.trim().length }],
						[/^[-*+] $/, 'bulletItem', {}],
						[/^\d+\. $/, 'orderedItem', { order: parseInt(combined) }],
						[/^\[ \] $/, 'todoItem', { checked: false }],
						[/^> $/, 'blockquote', {}],
						[/^```$/, 'codeBlock', {}]
					];
					const match = patterns.find(([pattern]) => pattern.test(combined));
					if (!match) {
						const wrapped = /(\*\*|~~|`|\*)([^*~`]+)\1$/.exec(combined);
						if (!wrapped) return false;
						const mark =
							wrapped[1] === '**'
								? 'bold'
								: wrapped[1] === '~~'
									? 'strike'
									: wrapped[1] === '`'
										? 'code'
										: 'italic';
						const start = $from.start() + wrapped.index;
						const value = wrapped[2];
						const tr = view.state.tr
							.insertText(value, start, from)
							.addMark(start, start + value.length, this.schema!.marks[mark].create());
						tr.removeStoredMark(this.schema!.marks[mark]);
						view.dispatch(tr);
						return true;
					}
					if (
						['orderedItem', 'todoItem', 'blockquote'].includes(match[1]) &&
						!this.mods!.loroPm.hasPageStructure(this.doc)
					) {
						const id = this.blockIdAtPos(from);
						void this.hooks.ensureStructure?.().then((ready) => {
							if (!ready || !id || !this.view) return;
							const block = this.blockPosById(this.view.state, id);
							if (!block) return;
							const current = this.view.state.doc.nodeAt(block.pos);
							if (!current?.textContent.startsWith(before)) return;
							this.view.dispatch(
								this.view.state.tr
									.delete(block.pos + 1, block.pos + 1 + before.length)
									.setNodeMarkup(block.pos, this.schema!.nodes[match[1]], {
										indent: current.attrs.indent,
										...match[2]
									})
							);
						});
						return true;
					}
					view.dispatch(
						view.state.tr
							.delete($from.start(), from)
							.setNodeMarkup($from.before(), this.schema!.nodes[match[1]], {
								indent: $from.parent.attrs.indent,
								...match[2]
							})
					);
					return true;
				},
				handleDOMEvents: {
					drop: (_view, event) => {
						if (
							(event as DragEvent).dataTransfer?.types.includes('application/x-sylvode-page-move')
						) {
							event.preventDefault();
							return true;
						}
						return false;
					},
					beforeinput: (view, event) => {
						if (!(event as InputEvent).isComposing) this.syncSelectionFromDOM(view);
						return false;
					},
					copy: (view, event) => this.writeBlockClipboard(view, event as ClipboardEvent, false),
					cut: (view, event) => this.writeBlockClipboard(view, event as ClipboardEvent, true)
				},
				handlePaste: (view, event, suppliedSlice) => {
					this.syncSelectionFromDOM(view);
					if (
						(this.plainPasteArmed || this.plainPasteUntil > Date.now()) &&
						this.editable &&
						!view.composing
					) {
						this.plainPasteUntil = 0;
						this.plainPasteArmed = false;
						this.hooks.onPasteModeChange?.(false);
						const text = event.clipboardData?.getData('text/plain') ?? '';
						if (!text) return true;
						this.insertPastedSlice(
							new this.mods!.pm.Slice(
								this.mods!.pm.Fragment.fromArray(plainPasteBlocks(text, view.state.schema)),
								0,
								0
							)
						);
						return true;
					}
					this.plainPasteUntil = 0;
					if (
						!this.mods!.loroPm.hasPageStructure(this.doc) &&
						!event.clipboardData?.getData('application/x-sylvode-page-blocks') &&
						event.clipboardData?.getData('text/html')
					) {
						let structural = false;
						suppliedSlice.content.descendants((node) => {
							if (
								['orderedItem', 'todoItem', 'blockquote', 'toggle', 'divider'].includes(
									node.type.name
								) ||
								Number(node.attrs.indent) > 0
							)
								structural = true;
						});
						if (structural) {
							this.insertPastedSlice(suppliedSlice);
							return true;
						}
					}
					const raw = event.clipboardData?.getData('application/x-sylvode-page-blocks');
					if (raw && this.editable && !view.composing) {
						if (raw.length > 1048576) {
							this.hooks.onClipboardError?.('paste');
							return true;
						}
						try {
							const parsed = JSON.parse(raw);
							if (parsed.version === 3 && parsed.cutTicket) {
								const target = this.blockIdAtPos(view.state.selection.from);
								const manager = this.mods!.loroPm.loroUndoPluginKey.getState(
									view.state
								)?.undoManager;
								if (!target || !manager || !Array.isArray(parsed.blocks))
									throw new Error('Invalid cut target');
								this.mods!.blockCut.pasteBlockCut(
									this.doc,
									parsed.cutTicket,
									parsed.blocks,
									target,
									manager
								);
								return true;
							}
							if (
								(parsed.version === 1 || parsed.version === 2) &&
								Array.isArray(parsed.blocks) &&
								parsed.blocks.length > 0 &&
								parsed.blocks.length <= 256
							) {
								const nodes = parsed.blocks.map((block: unknown) => {
									const node = pageClipboardNode(
										block,
										view.state.schema,
										parsed.version === 2 &&
											trustedBodyClipboard(this.doc, parsed.ticket, parsed.blocks)
									);
									return node;
								});
								this.insertPastedBlocks(nodes);
								return true;
							}
						} catch {
							this.hooks.onClipboardError?.('paste');
							return true;
						}
					}

					if (!this.editable || view.composing || event.clipboardData?.getData('text/html'))
						return false;
					const parsed = markdownBlocks(
						event.clipboardData?.getData('text/plain') ?? '',
						view.state.schema
					);
					if (!parsed) return false;
					this.insertPastedSlice(
						new this.mods!.pm.Slice(this.mods!.pm.Fragment.fromArray(parsed), 0, 0)
					);
					return true;
				},
				handleClickOn: (view, pos, node, nodePos, event, direct) => {
					if (!this.editable || !direct || !['todoItem', 'toggle'].includes(node.type.name))
						return false;
					const element = view.nodeDOM(nodePos) as HTMLElement | null;
					if (!element || event.clientX > element.getBoundingClientRect().left + 28) return false;
					const attr = node.type.name === 'todoItem' ? 'checked' : 'collapsed';
					view.dispatch(view.state.tr.setNodeAttribute(nodePos, attr, !node.attrs[attr]));
					return true;
				},
				decorations: (state) => {
					const hidden: import('prosemirror-view').Decoration[] = [];
					let collapsedIndent: number | null = null;
					const orderedAtDepth = new Map<number, number>();
					state.doc.forEach((node, pos) => {
						const identity = this.cursorFor(state, node, 0, 0)?.blockId;
						if (identity)
							hidden.push(
								pmView.Decoration.node(pos, pos + node.nodeSize, {
									'data-block-id': identity,
									'data-block-kind': node.type.name
								})
							);
						const indent = Number(node.attrs.indent) || 0;
						for (const depth of orderedAtDepth.keys())
							if (depth > indent) orderedAtDepth.delete(depth);
						if (node.type.name === 'orderedItem') {
							const number =
								(orderedAtDepth.get(indent) ?? (Number(node.attrs.order) || 1) - 1) + 1;
							orderedAtDepth.set(indent, number);
							hidden.push(
								pmView.Decoration.node(pos, pos + node.nodeSize, { 'data-order': String(number) })
							);
						} else orderedAtDepth.delete(indent);
						if (collapsedIndent !== null && indent <= collapsedIndent) collapsedIndent = null;
						if (collapsedIndent !== null)
							hidden.push(
								pmView.Decoration.node(pos, pos + node.nodeSize, {
									style: 'display:none',
									'aria-hidden': 'true'
								})
							);
						if (collapsedIndent === null && node.type.name === 'toggle' && node.attrs.collapsed)
							collapsedIndent = indent;
					});
					return pmView.DecorationSet.create(state.doc, hidden);
				}
			}
		});
	}

	async mount(host: HTMLElement, _blockId: string): Promise<void> {
		void _blockId;
		const mods = await loadEngineModules();
		if (this.destroyed) return;
		this.mods = mods;
		const { pm, pmState, pmView, pmCommands, pmKeymap, loroPm } = mods;
		const schema = buildSchema(pm);
		this.schema = schema;

		const nestOutdent =
			(delta: number): import('prosemirror-state').Command =>
			(_state, dispatch) =>
				dispatch ? this.adjustIndentAtPos(this.view?.state.selection.from ?? 0, delta) : true;

		const customKeymap = pmKeymap.keymap({
			Enter: (state, dispatch, view) => {
				if (view?.composing) return false;
				const parent = state.selection.$from.parent;
				if (
					!['bulletItem', 'orderedItem', 'todoItem', 'toggle', 'blockquote'].includes(
						parent.type.name
					)
				)
					return false;
				if (!parent.textContent)
					return pmCommands.setBlockType(schema.nodes.paragraph, {
						indent: Math.max(0, Number(parent.attrs.indent) - 1)
					})(state, dispatch, view);
				return pmCommands.splitBlockAs((node) => ({
					type: node.type,
					attrs: {
						...node.attrs,
						...(node.type.name === 'orderedItem' ? { order: Number(node.attrs.order) + 1 } : {}),
						...(node.type.name === 'todoItem' ? { checked: false } : {})
					}
				}))(state, dispatch, view);
			},
			'Alt-ArrowUp': (state) => this.moveBlockStep(state.selection.from, -1),
			'Alt-ArrowDown': (state) => this.moveBlockStep(state.selection.from, 1),
			'Mod-Enter': (state, dispatch) => {
				const { $from } = state.selection;
				const kind = $from.parent.type.name;
				if (!['todoItem', 'toggle'].includes(kind)) return false;
				const attr = kind === 'todoItem' ? 'checked' : 'collapsed';
				dispatch?.(state.tr.setNodeAttribute($from.before(), attr, !$from.parent.attrs[attr]));
				return true;
			},
			'Mod-b': pmCommands.toggleMark(schema.marks.bold),
			'Mod-i': pmCommands.toggleMark(schema.marks.italic),
			'Mod-Shift-s': pmCommands.toggleMark(schema.marks.strike),
			'Mod-Shift-x': pmCommands.toggleMark(schema.marks.strike),
			'Mod-e': pmCommands.toggleMark(schema.marks.code),
			'Mod-z': loroPm.undo,
			'Mod-y': loroPm.redo,
			'Mod-Shift-z': loroPm.redo,
			Tab: nestOutdent(1),
			'Shift-Tab': nestOutdent(-1)
		});
		const baseKeymapPlugin = pmKeymap.keymap(pmCommands.baseKeymap);

		let undoManager: import('loro-crdt').UndoManager;
		try {
			undoManager = await pageUndoManager(this.doc);
		} catch {
			if (this.destroyed) return;
			this.hooks.onInputError?.('This page session has closed. Reopen the page to continue.');
			return;
		}
		if (this.destroyed || !editorSessionAvailable(this.doc)) return;
		const state = pmState.EditorState.create({
			schema,
			doc: emptyDoc(schema),
			plugins: [
				loroPm.LoroSyncPlugin({ doc: this.doc }),
				loroPm.LoroUndoPlugin({ doc: this.doc, undoManager }),
				this.buildPresencePlugin(pmState, pmView),
				this.buildWritingPlugin(pmState, pmView),
				customKeymap,
				baseKeymapPlugin
			]
		});

		this.view = new pmView.EditorView(host, {
			state,
			nodeViews: this.hooks.body
				? Object.fromEntries(
						['image', 'attachment', 'childPage'].map((kind) => [
							kind,
							(
								node: import('prosemirror-model').Node,
								view: import('prosemirror-view').EditorView,
								getPos: () => number | undefined
							) =>
								mods.bodyViews.bodyNodeView(
									node,
									view,
									getPos,
									this.hooks.body!.objectId,
									this.hooks.body!.jobs,
									this.bodyEditable,
									{
										blockId: (pos) => this.blockIdAtPos(pos),
										replace: (id, kind, attrs) =>
											this.hooks.onBodyAction?.('replace', id, kind, attrs),
										details: (id, kind, attrs) =>
											this.hooks.onBodyAction?.('details', id, kind, attrs),
										continue: (pos) => this.continueAfterBlock(pos)
									}
								)
						])
					)
				: undefined,
			attributes: (state) => {
				const life = this.hooks.lifecycleInfo?.();
				return {
					role: 'textbox',
					'aria-label': 'Page editor',
					'aria-multiline': 'true',
					'data-testid': 'flow-page-editor',
					...editorSessionDiagnostic(this.doc),
					'data-editor-ref-count': String(life?.refs ?? -1),
					'data-editor-generation': String(life?.generation ?? -1),
					'data-editor-retired-count': String(life?.retired ?? -1),
					'data-editor-pm-anchor': String(state.selection.anchor),
					'data-editor-pm-head': String(state.selection.head)
				};
			},
			handleKeyDown: (_view, event) => {
				if (!event.isComposing) this.syncSelectionFromDOM(_view);
				if (event.key === 'Escape' && this.plainPasteArmed) {
					this.cancelPlainPaste(false);
					return true;
				}
				if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === 'v')
					this.plainPasteUntil = Date.now() + 1500;
				return (
					!event.isComposing && !this.view?.composing && (this.hooks.onKeyDown?.(event) ?? false)
				);
			},
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
				let newState: import('prosemirror-state').EditorState;
				try {
					newState = this.view.state.apply(tr);
				} catch (error) {
					if (error instanceof EditorWriteFailure) {
						this.editable = false;
						this.view.setProps({ editable: () => false });
					}
					this.view.updateState(this.view.state);
					this.hooks.onInputError?.(
						error instanceof Error ? error.message : 'This edit could not be applied.'
					);
					return;
				}
				const docChanged = this.view.state.doc !== newState.doc;
				this.view.updateState(newState);
				if (docChanged) this.hooks.onDocumentChange?.();
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
			}
		});
		host.addEventListener('focusout', (event) => {
			const next = event.relatedTarget as HTMLElement | null;
			if (!next?.closest?.('[data-flow-plain-paste-control]')) this.cancelPlainPaste(false);
			if (this.view) this.reportPresence(this.view.state);
		});
		await new Promise<void>((resolve) => setTimeout(resolve, 0));
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
		if (!state.selection.empty || !$from.parent.isTextblock) {
			this.hooks.onSlashMenu({ active: false, query: '', coords: null });
			return;
		}
		const textBefore = $from.parent.textBetween(0, $from.parentOffset, undefined, '￼');
		const match = /(?:^|\s)\/([^\s/]*)$/u.exec(textBefore);
		if (match && `${$from.pos}:${textBefore}` !== this.dismissedSlash) {
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

	dismissSlash(): void {
		if (!this.view) return;
		const { $from } = this.view.state.selection;
		this.dismissedSlash = `${$from.pos}:${$from.parent.textBetween(0, $from.parentOffset, undefined, '￼')}`;
	}

	/** Called by the slash menu UI when the user picks a block type; replaces the current block.
	 * Guarded by the renderer registry (`contracts/ui-surface-v1.md` "Renderer registry": "未注册
	 * type 失败为 read-only unsupported，不把 arbitrary component path 写入 CRDT") -- defense in
	 * depth alongside the caller's own `FlowBlockType`-typed union, since this is the one place a
	 * block type string reaches a CRDT-mutating transaction. */
	setBlockType(type: FlowBlockType, level?: number): void {
		if (
			['orderedItem', 'todoItem', 'blockquote', 'toggle', 'divider'].includes(type) &&
			!this.mods?.loroPm.hasPageStructure(this.doc)
		) {
			const saved = this.getSelection();
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && !this.destroyed) {
					if (saved) this.restoreSelection(saved);
					this.setBlockType(type, level);
				}
			});
			return;
		}
		if (!this.view || !this.schema || !isRegisteredBlockType(type)) return;
		const { state, dispatch } = this.view;
		const { $from } = state.selection;
		const pos = $from.before($from.depth);
		const nodeType = this.schema.nodes[type];
		const indent = $from.parent.attrs.indent as number;
		const attrs = type === 'heading' ? { indent, level: level ?? 1 } : { indent };
		const tr = state.tr;
		const before = $from.parent.textBetween(0, $from.parentOffset, undefined, '￼');
		const slash = /(?:^|\s)\/[^\s/]*$/u.exec(before);
		if (slash) {
			const start = $from.start() + slash.index + (slash[0].startsWith('/') ? 0 : 1);
			tr.delete(start, $from.pos);
		}
		if (type === 'divider') {
			tr.replaceWith(pos, pos + tr.doc.nodeAt(pos)!.nodeSize, [
				nodeType.create(attrs),
				this.schema.nodes.paragraph.create({ indent })
			]);
		} else tr.setNodeMarkup(pos, nodeType, attrs);

		dispatch(tr);
		this.view.focus();
	}

	/** Insert a sibling at a stable bookmark. An after bookmark skips the anchor's descendants. */
	async insertBlockType(
		target: import('./body/assets').BodyInsertionBookmark,
		type: FlowBlockType,
		level?: number
	): Promise<boolean> {
		if (!this.view || !this.editable || !this.schema || !this.mods || !isRegisteredBlockType(type))
			return false;
		if (
			['orderedItem', 'todoItem', 'blockquote', 'toggle', 'divider'].includes(type) &&
			!this.mods.loroPm.hasPageStructure(this.doc)
		) {
			if (!(await this.hooks.ensureStructure?.()) || !this.mods.loroPm.hasPageStructure(this.doc))
				return false;
			return this.insertBlockType(target, type, level);
		}
		const { state } = this.view;
		const anchor = target.blockId ? this.blockPosById(state, target.blockId) : null;
		const node = anchor ? state.doc.nodeAt(anchor.pos) : state.doc.firstChild;
		if (!node || (target.blockId && !anchor)) return false;
		if (
			!anchor &&
			(state.doc.childCount !== 1 || node.type.name !== 'paragraph' || node.content.size)
		)
			return false;

		const indent = Number(node.attrs.indent);
		let pos = anchor?.pos ?? 0;
		if (anchor && target.side === 'after') {
			pos += node.nodeSize;
			state.doc.forEach((sibling, offset) => {
				if (offset === pos && Number(sibling.attrs.indent) > indent) pos += sibling.nodeSize;
			});
		}
		const inserted = this.schema.nodes[type].create(
			type === 'heading' ? { indent, level: level ?? 1 } : { indent }
		);
		const tr = state.tr.replaceWith(pos, anchor ? pos : node.nodeSize, inserted);
		if (inserted.isTextblock) {
			tr.setSelection(this.mods.pmState.TextSelection.create(tr.doc, pos + 1));
		} else {
			tr.setSelection(this.mods.pmState.NodeSelection.create(tr.doc, pos));
		}
		this.view.dispatch(tr.scrollIntoView());
		this.view.focus();
		return (!anchor && type === 'paragraph') || !this.view.state.doc.eq(state.doc);
	}

	selectedBlockElement(): { pos: number; element: HTMLElement } | null {
		if (!this.view) return null;
		const { selection, doc } = this.view.state;
		const pos = selection.$head.depth
			? selection.$head.before(1)
			: doc.childAfter(selection.head).offset;
		const element = this.view.nodeDOM(pos);
		return element instanceof HTMLElement ? { pos, element } : null;
	}

	isEmptyPage(): boolean {
		const doc = this.view?.state.doc;
		return (
			doc?.childCount === 1 &&
			doc.firstChild?.type.name === 'paragraph' &&
			!doc.firstChild.content.size
		);
	}

	endBookmark(): import('./body/assets').BodyInsertionBookmark | null {
		if (!this.view) return null;
		const { doc } = this.view.state;
		if (this.isEmptyPage() && !this.blockIdAtPos(1)) return { blockId: null, side: 'before' };
		let root = 0;
		doc.forEach((node, pos) => {
			if (Number(node.attrs.indent) === 0) root = pos;
		});
		const blockId = this.blockIdAtPos(root);
		return blockId ? { blockId, side: 'after' } : null;
	}

	focusEditor(): void {
		this.view?.focus();
	}

	/** Removes the trailing "/query" that opened the slash menu without changing the block type.
	 * Used when the picked entry is not a text block (Collection, Forms reference/embed, convert),
	 * whose result is created by the server or the bridge rather than by this editor. */
	consumeSlashTrigger(): void {
		if (!this.view) return;
		const { state, dispatch } = this.view;
		const { $from } = state.selection;
		const before = $from.parent.textBetween(0, $from.parentOffset, undefined, '￼');
		const match = /(?:^|\s)\/[^\s/]*$/u.exec(before);
		if (!match) return;
		const start = $from.start() + match.index + (match[0].startsWith('/') ? 0 : 1);
		dispatch(state.tr.delete(start, $from.pos));
	}

	/** Backs the canvas's per-block hover handle nest/outdent buttons (task brief item 1's "block
	 * hover handle"), which act on whatever block the pointer is over rather than the current
	 * selection. Shares the same clamp-to-`MAX_INDENT` rule as the Tab/Shift-Tab keymap. */
	adjustIndentAtPos(pos: number, delta: number): boolean {
		if (!this.mods?.loroPm.hasPageStructure(this.doc)) {
			const id = this.blockIdAtPos(pos);
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && id && this.view) {
					const block = this.blockPosById(this.view.state, id);
					if (block) this.adjustIndentAtPos(block.pos + 1, delta);
				}
			});
			return true;
		}
		if (!this.view || !this.editable) return false;

		const { state } = this.view;
		const blocks: { node: import('prosemirror-model').Node; pos: number }[] = [];
		state.doc.forEach((node, offset) => blocks.push({ node, pos: offset }));
		const index = blocks.findIndex(
			(block) => pos >= block.pos && pos < block.pos + block.node.nodeSize
		);
		if (index < 0) return false;
		const depth = Number(blocks[index].node.attrs.indent) || 0;
		if (
			delta > 0 &&
			(index === 0 || depth >= (Number(blocks[index - 1].node.attrs.indent) || 0) + 1)
		)
			return false;
		if (delta < 0 && depth === 0) return false;
		let end = index + 1;
		while (end < blocks.length && Number(blocks[end].node.attrs.indent) > depth) end++;
		if (
			blocks.slice(index, end).some((block) => Number(block.node.attrs.indent) + delta > MAX_INDENT)
		)
			return false;
		const tr = state.tr;
		for (const block of blocks.slice(index, end))
			tr.setNodeAttribute(block.pos, 'indent', Number(block.node.attrs.indent) + delta);
		this.view.dispatch(tr);
		return true;
	}

	/** Maps a DOM point to a document position, for the hover handle to know which block the
	 * pointer is over. Returns `null` outside the editable area. */
	posAtClientPoint(x: number, y: number): number | null {
		if (!this.view) return null;
		return this.view.posAtCoords({ left: x, top: y })?.pos ?? null;
	}

	applyRemote(_change: EngineDiff): void {
		void _change;
		// `LoroSyncPlugin` subscribes to the bound Loro container itself and reconciles the
		// ProseMirror doc on every remote import automatically -- there is nothing this method
		// needs to do beyond existing for interface conformance
		// (`contracts/ui-surface-v1.md`'s `EditorAdapter.applyRemote`).
	}

	getSelection(): RelativeSelection | null {
		if (!this.view) return null;
		const { selection } = this.view.state;
		const anchorBlockId = this.blockIdAtPos(selection.anchor) ?? undefined;
		const headBlockId = this.blockIdAtPos(selection.head) ?? undefined;
		return {
			anchor: selection.anchor,
			head: selection.head,
			anchorBlockId,
			headBlockId,
			anchorOffset: selection.$anchor.parentOffset,
			headOffset: selection.$head.parentOffset
		};
	}

	restoreSelection(value: RelativeSelection): void {
		if (!this.view || !this.mods) return;
		const { pmState } = this.mods;
		const size = this.view.state.doc.content.size;
		const anchorBlock = value.anchorBlockId
			? this.blockPosById(this.view.state, value.anchorBlockId)
			: null;
		const headBlock = value.headBlockId
			? this.blockPosById(this.view.state, value.headBlockId)
			: null;
		const anchor = anchorBlock
			? anchorBlock.pos + 1 + Math.min(value.anchorOffset ?? 0, anchorBlock.size)
			: Math.min(Math.max(0, value.anchor), size);
		const head = headBlock
			? headBlock.pos + 1 + Math.min(value.headOffset ?? 0, headBlock.size)
			: Math.min(Math.max(0, value.head), size);
		const tr = this.view.state.tr.setSelection(
			pmState.TextSelection.create(this.view.state.doc, anchor, head)
		);
		this.view.dispatch(tr);
	}

	async moveBlockByIds(
		sourceId: string,
		targetId: string,
		placement: 'before' | 'after' | 'inside'
	): Promise<boolean> {
		if (!this.view || !this.editable || this.destroyed || !editorSessionAvailable(this.doc))
			return false;
		if (!this.mods?.loroPm.hasPageStructure(this.doc) && !(await this.hooks.ensureStructure?.()))
			return false;
		if (!this.view || !this.editable || this.destroyed || !editorSessionAvailable(this.doc))
			return false;
		const source = this.positionForBlock(sourceId),
			target = this.positionForBlock(targetId);
		return source !== null && target !== null && this.moveBlockAtPos(source, target, placement);
	}

	/** Inserts fresh blocks at the target's level, retaining same-page asset and link references. */
	async copyBlockByIds(
		sourceId: string,
		targetId: string,
		placement: 'before' | 'after'
	): Promise<boolean> {
		if (
			!this.view ||
			!this.mods ||
			!this.editable ||
			this.destroyed ||
			!editorSessionAvailable(this.doc)
		)
			return false;
		if (!this.mods.loroPm.hasPageStructure(this.doc) && !(await this.hooks.ensureStructure?.()))
			return false;
		if (!this.view || !this.editable || this.destroyed || !editorSessionAvailable(this.doc))
			return false;
		const { state } = this.view;
		const source = this.blockPosById(state, sourceId);
		const target = this.blockPosById(state, targetId);
		if (!source || !target) return false;
		const sourceNode = state.doc.nodeAt(source.pos)!;
		const targetNode = state.doc.nodeAt(target.pos)!;
		const sourceEnd = this.subtreeEnd(state.doc, source.pos, sourceNode);
		if (target.pos > source.pos && target.pos < sourceEnd) return false;
		const delta = Number(targetNode.attrs.indent) - Number(sourceNode.attrs.indent);
		const copies: import('prosemirror-model').Node[] = [];
		state.doc.slice(source.pos, sourceEnd).content.forEach((node) => {
			const json = node.toJSON();
			json.attrs.indent = Number(node.attrs.indent) + delta;
			copies.push(state.schema.nodeFromJSON(json));
		});
		if (copies.some((node) => Number(node.attrs.indent) > MAX_INDENT)) return false;
		const insertion =
			placement === 'before' ? target.pos : this.subtreeEnd(state.doc, target.pos, targetNode);
		try {
			this.view.dispatch(state.tr.insert(insertion, this.mods.pm.Fragment.fromArray(copies)));
			return this.view.state.doc !== state.doc;
		} catch {
			return false;
		}
	}

	private subtreeEnd(
		doc: import('prosemirror-model').Node,
		position: number,
		node: import('prosemirror-model').Node
	): number {
		let end = position + node.nodeSize;
		doc.nodesBetween(end, doc.content.size, (child, offset) => {
			if (offset === end && Number(child.attrs.indent) > Number(node.attrs.indent))
				end = offset + child.nodeSize;
			return false;
		});
		return end;
	}

	moveBlockAtPos(
		sourcePos: number,
		targetPos: number,
		placement: 'before' | 'after' | 'inside'
	): boolean {
		if (!this.view || !this.editable) return false;
		const sourceId = this.blockIdAtPos(sourcePos);
		const targetId = this.blockIdAtPos(targetPos);
		if (!sourceId || !targetId || sourceId === targetId) return false;
		if (!this.mods?.loroPm.hasPageStructure(this.doc)) {
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && this.view) {
					const source = this.blockPosById(this.view.state, sourceId);
					const target = this.blockPosById(this.view.state, targetId);
					if (source && target) this.moveBlockAtPos(source.pos + 1, target.pos + 1, placement);
				}
			});
			return true;
		}
		const entries = this.mods!.loroPm.structureEntries(this.doc);
		const sourceIndex = entries.findIndex((entry) => entry.block.id === sourceId);
		const targetIndex = entries.findIndex((entry) => entry.block.id === targetId);
		if (sourceIndex < 0 || targetIndex < 0) return false;
		const source = entries[sourceIndex],
			target = entries[targetIndex];
		let end = sourceIndex + 1;
		while (end < entries.length && entries[end].depth > source.depth) end++;
		if (targetIndex >= sourceIndex && targetIndex < end) return false;
		const depth = target.depth + (placement === 'inside' ? 1 : 0);
		const height = Math.max(
			...entries.slice(sourceIndex, end).map((entry) => entry.depth - source.depth)
		);
		if (depth + height > MAX_INDENT) return false;
		try {
			const manager = this.mods!.loroPm.loroUndoPluginKey.getState(this.view.state)?.undoManager;
			atomicEditorWrite(
				this.doc,
				() => {
					if (placement === 'before') source.tree.moveBefore(target.tree);
					else if (placement === 'after') source.tree.moveAfter(target.tree);
					else source.tree.move(target.tree, target.tree.children()?.length ?? 0);
					this.doc.commit({ origin: 'flow:structure' });
				},
				{ undoManager: manager, groupIntent: true }
			);
			return true;
		} catch {
			return false;
		}
	}
	moveBlockStep(pos: number, direction: -1 | 1): boolean {
		if (!this.view) return false;
		const id = this.blockIdAtPos(pos);
		if (!this.mods?.loroPm.hasPageStructure(this.doc)) {
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && id && this.view) {
					const block = this.blockPosById(this.view.state, id);
					if (block) this.moveBlockStep(block.pos + 1, direction);
				}
			});
			return true;
		}
		const entries = this.mods!.loroPm.structureEntries(this.doc);
		const index = entries.findIndex((entry) => entry.block.id === id);
		if (index < 0) return false;
		const source = entries[index];
		const siblings = entries.filter(
			(entry) => entry.tree.parent()?.id === source.tree.parent()?.id
		);
		const at = siblings.findIndex((entry) => entry.block.id === id);
		const target = siblings[at + direction];
		if (!target) return false;
		const block = this.blockPosById(this.view.state, target.block.id);
		return block
			? this.moveBlockAtPos(pos, block.pos + 1, direction === -1 ? 'before' : 'after')
			: false;
	}

	/** Internal whole-block copy inserts full sibling subtrees, never merges their text into a target. */
	private insertPastedBlocks(nodes: import('prosemirror-model').Node[], targetId?: string): void {
		if (!this.view || !this.editable || !this.mods) return;
		const target = targetId ?? this.blockIdAtPos(this.view.state.selection.head);
		if (!target) throw new Error('Missing paste position');
		const structural = nodes.some(
			(node) =>
				Number(node.attrs.indent) > 0 ||
				!['paragraph', 'heading', 'bulletItem', 'codeBlock'].includes(node.type.name)
		);
		if (structural && !this.mods.loroPm.hasPageStructure(this.doc)) {
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && this.view && !this.destroyed) {
					try {
						this.insertPastedBlocks(nodes, target);
					} catch {
						this.hooks.onClipboardError?.('paste');
					}
				}
			});
			return;
		}
		const block = this.blockPosById(this.view.state, target);
		if (!block) throw new Error('Paste target changed');
		const targetNode = this.view.state.doc.nodeAt(block.pos)!;
		const depth = Number(targetNode.attrs.indent);
		let end = block.pos + targetNode.nodeSize;
		this.view.state.doc.forEach((node, pos) => {
			if (pos === end && Number(node.attrs.indent) > depth) end = pos + node.nodeSize;
		});
		const levels: { source: number; relative: number }[] = [];
		const copies = nodes.map((node) => {
			const source = Number(node.attrs.indent);
			while (levels.length && levels[levels.length - 1].source >= source) levels.pop();
			const relative = levels.length ? levels[levels.length - 1].relative + 1 : 0;
			if (depth + relative > MAX_INDENT) throw new Error('Paste exceeds hierarchy depth');
			levels.push({ source, relative });
			return node.type.create(
				{ ...node.attrs, indent: depth + relative },
				node.content,
				node.marks
			);
		});
		const tr = this.view.state.tr.insert(end, this.mods.pm.Fragment.fromArray(copies));
		tr.setSelection(this.mods.pmState.Selection.near(tr.doc.resolve(end)));
		this.view.dispatch(tr.scrollIntoView());
	}

	private insertPastedSlice(slice: import('prosemirror-model').Slice): void {
		if (!this.view || !this.editable) return;
		let structural = false;
		slice.content.descendants((node) => {
			if (
				['orderedItem', 'todoItem', 'blockquote', 'toggle', 'divider'].includes(node.type.name) ||
				Number(node.attrs.indent) > 0
			)
				structural = true;
		});
		if (structural && !this.mods!.loroPm.hasPageStructure(this.doc)) {
			const saved = this.getSelection();
			void this.hooks.ensureStructure?.().then((ready) => {
				if (ready && this.view && !this.destroyed) {
					if (saved) this.restoreSelection(saved);
					this.insertPastedSlice(slice);
				}
			});
			return;
		}
		this.view.dispatch(this.view.state.tr.replaceSelection(slice).scrollIntoView());
	}

	positionForBlock(blockId: string): number | null {
		if (!this.view) return null;
		const block = this.blockPosById(this.view.state, blockId);
		if (!block) return null;
		return block.pos + (this.view.state.doc.nodeAt(block.pos)?.isTextblock ? 1 : 0);
	}
	bodyBookmark(allowEmpty = false): import('./body/assets').BodyInsertionBookmark | null {
		if (!this.view) return null;
		this.syncSelectionFromDOM(this.view);
		const blockId = this.blockIdAtPos(this.view.state.selection.head);
		if (blockId) return { blockId, side: 'after' };
		const { doc } = this.view.state;
		const first = doc.firstChild;
		return allowEmpty &&
			doc.childCount === 1 &&
			first?.type.name === 'paragraph' &&
			!first.content.size
			? { blockId: null, side: 'before' }
			: null;
	}
	continueAfterBlock(pos: number): void {
		if (!this.view || !this.editable || !this.schema || !this.mods) return;
		const node = this.view.state.doc.nodeAt(pos);
		if (!node) return;
		const end = this.subtreeEnd(this.view.state.doc, pos, node);
		const tr = this.view.state.tr.insert(
			end,
			this.schema.nodes.paragraph.create({ indent: node.attrs.indent })
		);
		tr.setSelection(this.mods.pmState.TextSelection.create(tr.doc, end + 1));
		this.view.dispatch(tr);
		this.view.focus();
	}

	blockAction(pos: number, action: 'duplicate' | 'delete' | 'copy' | 'cut' | 'select'): void {
		if (!this.view || !this.mods || !this.editable) return;

		const { state } = this.view;
		const blocks: { node: import('prosemirror-model').Node; pos: number }[] = [];
		state.doc.forEach((node, offset) => blocks.push({ node, pos: offset }));
		const index = blocks.findIndex(
			(block) => pos >= block.pos && pos < block.pos + block.node.nodeSize
		);
		if (index < 0) return;
		const block = blocks[index];
		let endIndex = index + 1;
		while (
			endIndex < blocks.length &&
			Number(blocks[endIndex].node.attrs.indent) > Number(block.node.attrs.indent)
		)
			endIndex++;
		const end = endIndex < blocks.length ? blocks[endIndex].pos : state.doc.content.size;
		const tr = state.tr;
		if (action === 'duplicate') {
			const copies: import('prosemirror-model').Node[] = [];
			state.doc
				.slice(block.pos, end)
				.content.forEach((node) => copies.push(this.schema!.nodeFromJSON(node.toJSON())));
			tr.insert(end, this.mods.pm.Fragment.fromArray(copies));
		} else if (action === 'delete') {
			tr.delete(block.pos, end);
			if (!tr.doc.childCount) tr.insert(0, this.schema!.nodes.paragraph.create({ indent: 0 }));
		} else if (action === 'select')
			tr.setSelection(
				this.mods.pmState.TextSelection.create(
					state.doc,
					block.pos + 1,
					Math.max(block.pos + 1, end - 1)
				)
			);
		else tr.setSelection(this.mods.pmState.NodeSelection.create(state.doc, block.pos));
		this.view.dispatch(tr);
		this.view.focus();
		if (action === 'copy' || action === 'cut') {
			this.copiedBlockRange = { from: block.pos, to: end };
			try {
				if (!document.execCommand(action) && action === 'cut') this.hooks.onClipboardError?.('cut');
			} finally {
				this.copiedBlockRange = null;
			}
		}
	}

	private writeBlockClipboard(
		view: import('prosemirror-view').EditorView,
		event: ClipboardEvent,
		cut: boolean
	): boolean {
		const clipboard = event.clipboardData;
		if (!clipboard || !this.mods || !this.schema) return false;
		if (!this.copiedBlockRange) this.syncSelectionFromDOM(view);
		let { from, to } = this.copiedBlockRange ?? view.state.selection;
		if (!this.copiedBlockRange && from !== to) {
			const { $from, $to } = view.state.selection;
			if (
				$from.depth === 1 &&
				$to.depth === 1 &&
				$from.parentOffset === 0 &&
				$to.parentOffset === $to.parent.content.size
			) {
				from = $from.before(1);
				to = $to.after(1);
			}
		}
		const blocks: unknown[] = [],
			ids: string[] = [];
		let start = -1,
			end = -1;
		view.state.doc.forEach((node, pos) => {
			if (pos >= from && pos + node.nodeSize <= to) {
				if (start < 0) start = pos;
				end = pos + node.nodeSize;
				blocks.push(node.toJSON());
				const id = this.blockIdAtPos(pos);
				if (id) ids.push(id);
			}
		});
		if (!blocks.length || start !== from || end !== to) return false;
		if (cut && (!this.editable || view.composing)) return true;
		try {
			const manager = this.mods.loroPm.loroUndoPluginKey.getState(view.state)?.undoManager;
			if (cut && !manager)
				throw new Error('Use Cut block from the block menu to prepare this Page.');
			const ticket = cut
				? this.mods.blockCut.prepareBlockCut(this.doc, ids, blocks)
				: issueBodyClipboard(this.doc, blocks);
			const slice = view.state.doc.slice(from, to);
			const wrapper = document.createElement('div');
			wrapper.append(
				this.mods.pm.DOMSerializer.fromSchema(view.state.schema).serializeFragment(slice.content)
			);
			clipboard.setData('text/html', wrapper.innerHTML);
			clipboard.setData('text/plain', slice.content.textBetween(0, slice.content.size, '\n'));
			const internal = JSON.stringify(
				cut ? { version: 3, cutTicket: ticket, blocks } : { version: 2, ticket, blocks }
			);
			if (internal.length > 1048576) throw new Error('Clipboard content exceeds limit');
			clipboard.setData('application/x-sylvode-page-blocks', internal);
			event.preventDefault();
			if (cut) {
				const apply = () => {
					if (!this.editable || this.destroyed || !editorSessionAvailable(this.doc)) return;
					try {
						this.mods!.blockCut.applyBlockCut(
							this.doc,
							ticket,
							blocks,
							manager!,
							this.schema!.nodes.paragraph.create({ indent: 0 })
						);
					} catch {
						this.hooks.onClipboardError?.('cut');
					}
				};
				if (this.mods.loroPm.hasPageStructure(this.doc)) apply();
				else
					void this.hooks.ensureStructure?.().then((ready) => {
						if (ready) apply();
					});
			}
			return true;
		} catch {
			event.preventDefault();
			this.hooks.onClipboardError?.('cut');
			return true;
		}
	}

	headings(): { pos: number; level: number; text: string }[] {
		const result: { pos: number; level: number; text: string }[] = [];
		if (this.view)
			visiblePageBlocks(this.view.state.doc, (node, pos) => {
				if (node.type.name === 'heading')
					result.push({ pos, level: Number(node.attrs.level), text: node.textContent });
			});
		return result;
	}

	selectPosition(pos: number): void {
		if (!this.view || !this.mods) return;
		const { state } = this.view;
		const block = this.view.nodeDOM(pos);
		if (!(block instanceof HTMLElement) || !state.doc.nodeAt(pos)?.isTextblock) return;
		if (this.editable) {
			this.view.dispatch(
				state.tr.setSelection(this.mods.pmState.TextSelection.create(state.doc, pos + 1))
			);
			this.view.focus();
		} else {
			block.tabIndex = -1;
			block.focus({ preventScroll: true });
		}
		block.scrollIntoView({ block: 'center', inline: 'nearest' });
	}

	findText(query: string, direction?: 1 | -1): { current: number; total: number } {
		if (this.findQuery !== query) {
			this.findIndex = -1;
			this.findDoc = undefined;
		}
		this.findQuery = query;
		return this.updateFind(direction, true);
	}

	refreshFind(): { current: number; total: number } {
		return this.updateFind();
	}

	private updateFind(direction?: 1 | -1, scroll = false): { current: number; total: number } {
		if (!this.view || !this.mods) return { current: 0, total: 0 };
		const view = this.view;
		const { Decoration, DecorationSet } = this.mods.pmView;
		if (this.findDoc !== view.state.doc) {
			this.findDoc = view.state.doc;
			this.findMatches = pageTextMatches(this.findDoc, this.findQuery);
		}
		const total = this.findMatches.length;
		if (!total) this.findIndex = -1;
		else if (this.findIndex < 0) this.findIndex = direction === -1 ? total - 1 : 0;
		else if (direction) this.findIndex = (this.findIndex + direction + total) % total;
		else this.findIndex = Math.min(this.findIndex, total - 1);
		const active = this.findIndex;
		const decorations = DecorationSet.create(
			view.state.doc,
			this.findMatches.map(({ from, to }, index) =>
				Decoration.inline(from, to, {
					class: index === active ? 'flow-find-match flow-find-current' : 'flow-find-match'
				})
			)
		);
		// View-only decorations leave the editor selection, CRDT, and undo history alone.
		const doc = view.state.doc;
		view.setProps({
			decorations: (state) => (state.doc === doc ? decorations : DecorationSet.empty)
		});
		if (active >= 0 && scroll)
			view.dom
				.querySelector('.flow-find-current')
				?.scrollIntoView({ block: 'center', inline: 'nearest' });
		return { current: active + 1, total };
	}

	replaceFound(replacement: string): boolean {
		const match = this.findMatches[this.findIndex];
		if (
			!this.view ||
			!this.editable ||
			!this.findQuery ||
			this.findDoc !== this.view.state.doc ||
			!match
		)
			return false;
		const { state } = this.view;
		this.view.dispatch(state.tr.insertText(replacement, match.from, match.to));
		return this.view.state.doc !== state.doc;
	}

	cancelPlainPaste(refocus = true): void {
		this.plainPasteArmed = false;
		this.plainPasteUntil = 0;
		this.hooks.onPasteModeChange?.(false);
		if (refocus) this.view?.focus();
	}

	togglePlainPaste(): void {
		if (!this.view || !this.editable) return;
		this.syncSelectionFromDOM(this.view);
		this.plainPasteArmed = !this.plainPasteArmed;
		this.plainPasteUntil = 0;
		this.hooks.onPasteModeChange?.(this.plainPasteArmed);
		this.view.focus();
	}

	toggleMark(mark: FlowMark): void {
		if (!this.view || !this.mods || !this.schema || !this.editable) return;
		this.mods.pmCommands.toggleMark(this.schema.marks[mark])(this.view.state, this.view.dispatch);
		this.view.focus();
	}
	setLink(href: string): boolean {
		if (!this.view || !this.schema || !this.editable || (href !== '' && !safeEditorHref(href)))
			return false;
		const { state } = this.view;
		const tr = state.tr.removeMark(
			state.selection.from,
			state.selection.to,
			this.schema.marks.link
		);
		if (href)
			tr.addMark(state.selection.from, state.selection.to, this.schema.marks.link.create({ href }));
		this.view.dispatch(tr);
		this.view.focus();
		return true;
	}
	selectionInfo(): {
		empty: boolean;
		marks: string[];
		linkHref?: string;
		left: number;
		top: number;
	} | null {
		if (!this.view) return null;
		const { state } = this.view;
		const coords = this.view.coordsAtPos(state.selection.from);
		return {
			empty: state.selection.empty,
			linkHref: state.selection.$from.marks().find((mark) => mark.type.name === 'link')?.attrs
				.href as string | undefined,
			marks: (state.storedMarks ?? state.selection.$from.marks()).map((m) => m.type.name),
			left: coords.left,
			top: coords.top
		};
	}
	focus(): void {
		this.view?.focus();
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
		this.destroyed = true;
		this.view?.destroy();
		this.view = null;
	}
}
