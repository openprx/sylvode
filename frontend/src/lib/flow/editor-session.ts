import { registerEditorUndoPolicy } from './editor-batch';
/** View-only caret survives Tab unmounts in the current account's shared document lifetime. */
import type { RelativeSelection } from './types';
const selections = new WeakMap<object, RelativeSelection>();
export function rememberEditorSelection(doc: object, selection: RelativeSelection): void {
	selections.set(doc, selection);
}
export function recalledEditorSelection(doc: object): RelativeSelection | undefined {
	return selections.get(doc);
}
import type { LoroDoc, UndoManager } from 'loro-crdt';
const histories = new WeakMap<object, UndoManager>();
const closedDocs = new WeakSet<object>();
export function editorSessionAvailable(doc: object): boolean {
	return !closedDocs.has(doc);
}
export async function pageUndoManager(doc: LoroDoc): Promise<UndoManager> {
	if (closedDocs.has(doc)) throw new Error('Document closed');
	const current = histories.get(doc);
	if (current) return current;
	const { UndoManager } = await import('loro-crdt');
	// Recheck after dynamic import: simultaneous view mounts share one manager.
	if (closedDocs.has(doc)) throw new Error('Document closed');
	const existing = histories.get(doc);
	if (existing) return existing;
	const manager = new UndoManager(doc, {});
	registerEditorUndoPolicy(manager, 1000);
	histories.set(doc, manager);
	return manager;
}
export function releaseEditorSession(doc: object): void {
	closedDocs.add(doc);
	histories.get(doc)?.free();
	histories.delete(doc);
	selections.delete(doc);
}

/** Detach callbacks before retiring a failed engine; free only after its view has unmounted. */
export function detachEditorUndo(doc: object): void {
	const manager = histories.get(doc);
	manager?.setOnPop();
	manager?.setOnPush();
}

const instanceIDs = new WeakMap<object, number>();
let nextInstanceID = 1;
function instanceID(value: object): string {
	let id = instanceIDs.get(value);
	if (!id) {
		id = nextInstanceID++;
		instanceIDs.set(value, id);
	}
	return String(id);
}
/** Stable object-instance diagnostics distinguish a remount from replacing its CRDT/UndoManager. */
export function editorSessionDiagnostic(doc: object): Record<string, string> {
	const manager = histories.get(doc);
	return {
		'data-editor-doc-instance': instanceID(doc),
		'data-editor-undo-instance': manager ? instanceID(manager) : 'none',
		'data-editor-can-undo': String(manager?.canUndo() ?? false),
		'data-editor-can-redo': String(manager?.canRedo() ?? false)
	};
}

/** Acknowledged server transfer boundary only. This never creates an Undo item or mutates content. */
export function clearEditorSessionHistory(doc: object): boolean {
	const history = histories.get(doc);
	if (!history) return false;
	history.clear();
	return true;
}
