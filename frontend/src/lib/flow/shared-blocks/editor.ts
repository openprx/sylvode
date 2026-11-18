/** A local single-member draft editor. Never accepts a source snapshot, Page ID, session or WS. */
import type { Node, Schema } from 'prosemirror-model';
import type { PageInlineSpan } from '../inline-runs';
import { pageInlineRuns } from '../inline-runs';
import { createPageSchema, safeEditorHref, type FlowMark } from '../editor-schema';
import type { SharedContent } from './contracts';
import { registerEditorUndoPolicy } from '../editor-batch';
import { configLoroTextStyle } from '../vendor/loro-prosemirror/text-style';
export function sharedEditorNode(schema: Schema, content: SharedContent): Node {
	if (!('text' in content)) throw new Error('shared_member_not_text');
	const nodes =
		content.kind === 'code'
			? content.text
				? [schema.text(content.text)]
				: []
			: pageInlineRuns(content.text, 'spans' in content ? content.spans : []).map((run) =>
					schema.text(
						run.text,
						run.marks.map((mark) =>
							schema.mark(mark.mark, mark.mark === 'link' ? { href: mark.href } : undefined)
						)
					)
				);
	return schema.nodes[content.kind === 'code' ? 'codeBlock' : 'paragraph'].create(
		{ indent: 0 },
		nodes
	);
}
export function sharedEditorValue(node: Node): { text: string; spans: PageInlineSpan[] } {
	let text = '',
		offset = 0;
	const spans: PageInlineSpan[] = [];
	node.forEach((child) => {
		if (!child.isText) throw new Error('shared_member_not_text');
		const value = child.text ?? '',
			end = offset + new TextEncoder().encode(value).length;
		for (const mark of child.marks)
			spans.push({
				start: offset,
				end,
				mark: mark.type.name as FlowMark,
				...(mark.type.name === 'link' ? { href: mark.attrs.href } : {})
			});
		text += value;
		offset = end;
	});
	return { text, spans };
}
let loading: Promise<{
	pm: typeof import('prosemirror-model');
	state: typeof import('prosemirror-state');
	view: typeof import('prosemirror-view');
	commands: typeof import('prosemirror-commands');
	keymap: typeof import('prosemirror-keymap');
	loro: typeof import('loro-crdt');
	binding: typeof import('../vendor/loro-prosemirror');
}> | null = null;
async function modules() {
	loading ??= Promise.all([
		import('prosemirror-model'),
		import('prosemirror-state'),
		import('prosemirror-view'),
		import('prosemirror-commands'),
		import('prosemirror-keymap'),
		import('loro-crdt'),
		import('../vendor/loro-prosemirror')
	]).then(([pm, state, view, commands, keymap, loro, binding]) => ({
		pm,
		state,
		view,
		commands,
		keymap,
		loro,
		binding
	}));
	return loading;
}
export class SharedMemberEditor {
	private view: import('prosemirror-view').EditorView | null = null;
	private doc: import('loro-crdt').LoroDoc | null = null;
	private undoManager: import('loro-crdt').UndoManager | null = null;
	private loaded: Awaited<ReturnType<typeof modules>> | null = null;
	private destroyed = false;
	private editable = true;
	private schema: Schema | null = null;
	private selection = { anchor: 1, head: 1 };
	private mounting = 0;
	private selected: () => void = () => {};
	constructor(
		private readonly content: SharedContent,
		private readonly changed: (value: { text: string; spans: PageInlineSpan[] }) => void,
		private readonly rejected: (reason: 'structure') => void = () => {},
		private readonly label = 'Shared block text',
		private readonly selectionChanged: () => void = () => {}
	) {}
	async mount(host: HTMLElement): Promise<void> {
		const generation = ++this.mounting;
		const m = await modules();
		if (this.destroyed || generation !== this.mounting) return;
		this.loaded = m;
		const code = this.content.kind === 'code';
		if (!this.doc || !this.schema || !this.undoManager) {
			const base = createPageSchema(m.pm);
			this.schema = new m.pm.Schema({
				nodes: base.spec.nodes.update('doc', { content: code ? 'codeBlock' : 'paragraph' }),
				marks: base.spec.marks
			});
			const initial = sharedEditorNode(this.schema, this.content);
			this.doc = new m.loro.LoroDoc();
			configLoroTextStyle(this.doc, this.schema);
			m.binding.updateLoroToPmState(
				this.doc as import('loro-prosemirror').LoroDocType,
				new Map(),
				m.state.EditorState.create({
					schema: this.schema,
					doc: this.schema.nodes.doc.create(null, initial)
				})
			);
			this.undoManager = new m.loro.UndoManager(this.doc, {});
			registerEditorUndoPolicy(this.undoManager);
		}
		const schema = this.schema,
			doc = this.doc,
			manager = this.undoManager;
		const current = m.binding.createNodeFromLoroObj(
			schema,
			doc.getMap('doc') as import('../vendor/loro-prosemirror').LoroNode,
			new Map()
		) as Node;

		const keys = m.keymap.keymap({
			'Mod-z': m.binding.undo,
			'Mod-Shift-z': m.binding.redo,
			'Mod-y': m.binding.redo,
			'Mod-b': code ? () => true : m.commands.toggleMark(schema.marks.bold),
			'Mod-i': code ? () => true : m.commands.toggleMark(schema.marks.italic),
			'Mod-Shift-s': code ? () => true : m.commands.toggleMark(schema.marks.strike),
			'Mod-e': code ? () => true : m.commands.toggleMark(schema.marks.code),
			Enter: (state, dispatch, view) => {
				if (view?.composing) return false;
				dispatch?.(state.tr.insertText('\n'));
				return true;
			},
			Backspace: (state) => state.selection.empty && state.selection.$from.parentOffset === 0,
			Delete: (state) =>
				state.selection.empty &&
				state.selection.$to.parentOffset === state.selection.$to.parent.content.size
		});
		const state = m.state.EditorState.create({
			schema,
			doc: current,
			selection: m.state.TextSelection.create(
				current,
				Math.min(this.selection.anchor, current.content.size - 1),
				Math.min(this.selection.head, current.content.size - 1)
			),
			plugins: [
				m.binding.LoroSyncPlugin({ doc: doc as import('loro-prosemirror').LoroDocType }),
				m.binding.LoroUndoPlugin({
					doc: doc as import('loro-prosemirror').LoroDocType,
					undoManager: manager
				}),
				keys,
				m.keymap.keymap(m.commands.baseKeymap)
			]
		});
		const view = new m.view.EditorView(host, {
			state,
			editable: () => this.editable,
			attributes: {
				role: 'textbox',
				'aria-multiline': 'true',
				'aria-label': this.label,
				'data-testid': 'flow-shared-member-editor'
			},
			dispatchTransaction: (tr) => {
				if (this.destroyed) return;
				view.updateState(view.state.applyTransaction(tr).state);
				if (tr.docChanged) this.changed(sharedEditorValue(view.state.doc.firstChild!));
				this.selectionChanged();
				this.selected();
			},
			handleDOMEvents: {
				drop: (_view, event) => {
					event.preventDefault();
					this.rejected('structure');
					return true;
				}
			},
			handlePaste: (view, event, slice) => {
				const inline: Node[] = [];
				if (event.clipboardData?.getData('application/x-sylvode-page-blocks')) {
					this.rejected('structure');
					return true;
				}
				if (!event.clipboardData?.getData('text/html')) {
					const plain = event.clipboardData?.getData('text/plain') ?? '';
					if (plain) inline.push(schema.text(plain.replace(/\r\n?/g, '\n')));
				} else {
					if (
						slice.content.childCount > 1 ||
						slice.content.content.some((node) => !node.isText && !node.isTextblock)
					) {
						this.rejected('structure');
						return true;
					}
					slice.content.forEach((node) => {
						if (node.isText) inline.push(code ? schema.text(node.text!) : node);
						else
							node.forEach((child) => {
								if (child.isText) inline.push(code ? schema.text(child.text!) : child);
							});
					});
				}
				view.dispatch(
					view.state.tr.replaceSelection(new m.pm.Slice(m.pm.Fragment.fromArray(inline), 0, 0))
				);
				return true;
			}
		});
		this.view = view;
	}
	value(): { text: string; spans: PageInlineSpan[] } | null {
		if (this.view) return sharedEditorValue(this.view.state.doc.firstChild!);
		if (this.doc && this.schema && this.loaded)
			return sharedEditorValue(
				(
					this.loaded.binding.createNodeFromLoroObj(
						this.schema,
						this.doc.getMap('doc') as import('../vendor/loro-prosemirror').LoroNode,
						new Map()
					) as Node
				).firstChild!
			);
		return null;
	}
	mark(mark: Exclude<FlowMark, 'link'>): void {
		if (!this.view || !this.editable || !this.loaded || this.content.kind === 'code') return;
		this.syncSelection();
		this.loaded.commands.toggleMark(this.view.state.schema.marks[mark])(
			this.view.state,
			this.view.dispatch,
			this.view
		);
		this.view.focus();
	}
	link(href: string | null): boolean {
		if (
			!this.view ||
			!this.editable ||
			this.content.kind === 'code' ||
			(href !== null && !safeEditorHref(href))
		)
			return false;
		this.syncSelection();
		const { state } = this.view;
		if (state.selection.empty) return false;
		const tr = state.tr.removeMark(
			state.selection.from,
			state.selection.to,
			state.schema.marks.link
		);
		if (href !== null)
			tr.addMark(
				state.selection.from,
				state.selection.to,
				state.schema.marks.link.create({ href })
			);
		this.view.dispatch(tr);
		this.view.focus();
		return true;
	}
	undo(): void {
		if (this.view && this.editable && this.loaded)
			this.loaded.binding.undo(this.view.state, this.view.dispatch, this.view);
	}
	redo(): void {
		if (this.view && this.editable && this.loaded)
			this.loaded.binding.redo(this.view.state, this.view.dispatch, this.view);
	}
	isComposing(): boolean {
		return this.view?.composing ?? false;
	}

	setEditable(value: boolean): void {
		this.editable = value;
		this.view?.setProps({ editable: () => this.editable });
	}
	private syncSelection(): void {
		const view = this.view,
			m = this.loaded;
		if (!view || !m || view.composing) return;
		const native = view.dom.ownerDocument.getSelection();
		if (
			!native?.anchorNode ||
			!native.focusNode ||
			!view.dom.contains(native.anchorNode) ||
			!view.dom.contains(native.focusNode)
		)
			return;
		try {
			const anchor = view.posAtDOM(native.anchorNode, native.anchorOffset),
				head = view.posAtDOM(native.focusNode, native.focusOffset);
			const selection = m.state.TextSelection.create(view.state.doc, anchor, head);
			if (!selection.eq(view.state.selection)) view.dispatch(view.state.tr.setSelection(selection));
		} catch {
			/* An outside or detached DOM caret is not a draft command. */
		}
	}
	onSelection(listener: () => void): void {
		this.selected = listener;
	}

	activeMarks(): string[] {
		if (!this.view) return [];
		const state = this.view.state;
		return (state.storedMarks ?? state.selection.$from.marks()).map((mark) => mark.type.name);
	}
	unmount(): void {
		this.mounting++;
		if (this.view)
			this.selection = {
				anchor: this.view.state.selection.anchor,
				head: this.view.state.selection.head
			};
		this.view?.destroy();
		this.view = null;
		this.selected = () => {};
	}
	destroy(): void {
		this.destroyed = true;
		this.unmount();
		this.undoManager?.free();
		this.undoManager = null;
		this.doc?.free();
		this.doc = null;
	}
}
