import { privateBodyAssetProvider } from './providers';
/** Mounted atom UI fetches bytes through the owning Page API; native document attrs contain no URL. */
import type { Node } from 'prosemirror-model';
import type { EditorView, NodeView } from 'prosemirror-view';
import type { PageBodyJobs } from './jobs';
import { writable, type Readable } from 'svelte/store';
import { mount, unmount } from 'svelte';
import FlowBodyNode from '#lib/components/flow/FlowBodyNode.svelte';
export interface BodyNodeActions {
	replace(blockId: string, kind: 'image' | 'attachment', attrs: Record<string, unknown>): void;
	details(blockId: string, kind: 'image' | 'attachment', attrs: Record<string, unknown>): void;
	continue(pos: number): void;
	blockId(pos: number): string | null;
}
export function bodyNodeView(
	node: Node,
	view: EditorView,
	getPos: () => number | undefined,
	objectId: string,
	jobs: PageBodyJobs | null,
	editable: Readable<boolean>,
	actions: BodyNodeActions
): NodeView {
	const dom = document.createElement('div');
	dom.contentEditable = 'false';
	dom.draggable = false;
	dom.className = 'flow-body-atom';
	dom.dataset.indent = String(node.attrs.indent ?? 0);
	dom.style.marginLeft = `${Number(node.attrs.indent ?? 0) * 1.5}rem`;
	let current = node;
	const source = writable({
		kind: node.type.name,
		objectId,
		attrs: node.attrs,
		readOnly: !view.editable
	});
	const identity = () => {
		const pos = getPos();
		return pos === undefined ? null : actions.blockId(pos);
	};
	const component = mount(FlowBodyNode, {
		target: dom,
		props: {
			source,
			provider: privateBodyAssetProvider(objectId),
			jobs,
			editable,
			onReplace: () => {
				const id = identity();
				if (id) actions.replace(id, current.type.name as 'image' | 'attachment', current.attrs);
			},
			onDetails: () => {
				const id = identity();
				if (id) actions.details(id, current.type.name as 'image' | 'attachment', current.attrs);
			},
			onContinue: () => {
				const pos = getPos();
				if (pos !== undefined) actions.continue(pos);
			}
		}
	});
	return {
		dom,
		update(next) {
			if (next.type !== current.type) return false;
			current = next;
			dom.dataset.indent = String(next.attrs.indent ?? 0);
			dom.style.marginLeft = `${Number(next.attrs.indent ?? 0) * 1.5}rem`;
			source.set({ kind: next.type.name, objectId, attrs: next.attrs, readOnly: !view.editable });
			return true;
		},
		stopEvent: (event) =>
			!!(event.target as HTMLElement)?.closest('button,a,input,textarea,select'),
		ignoreMutation: () => true,
		selectNode: () => dom.classList.add('ProseMirror-selectednode'),
		deselectNode: () => dom.classList.remove('ProseMirror-selectednode'),
		destroy: () => {
			void unmount(component);
		}
	};
}
