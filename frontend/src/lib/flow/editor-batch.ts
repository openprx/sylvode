/** Synchronous editor transactions: intermediate commits never reach outbox or application reads. */
import type { LoroDoc, UndoManager, VersionVector } from 'loro-crdt';
export class EditorPreflightError extends Error {}
export class EditorWriteFailure extends Error {
	constructor(
		message: string,
		public readonly cause: unknown
	) {
		super(message);
	}
}
interface Batch {
	from: VersionVector;
	observers: Map<object, () => void>;
	sinks: Set<(bytes: Uint8Array) => void>;
}
const active = new WeakMap<object, Batch>();
const failed = new WeakSet<object>();
const pausedWrites = new WeakSet<object>();
export function pauseEditorWrites(doc: object): void {
	pausedWrites.add(doc);
}
const transientHolds = new WeakMap<object, Set<string>>();
export function editorWritesAllowed(doc: object): boolean {
	return !failed.has(doc) && !pausedWrites.has(doc) && !transientHolds.get(doc)?.size;
}
export function holdEditorWrites(doc: object, id: string): () => void {
	let holds = transientHolds.get(doc);
	if (!holds) {
		holds = new Set();
		transientHolds.set(doc, holds);
	}
	holds.add(id);
	return () => {
		holds!.delete(id);
		if (!holds!.size) transientHolds.delete(doc);
	};
}

const recoveries = new WeakMap<object, (preimage: LoroDoc, error: unknown) => void>();
export function registerEditorRecovery(
	doc: object,
	recover: (preimage: LoroDoc, error: unknown) => void
): () => void {
	recoveries.set(doc, recover);
	return () => recoveries.delete(doc);
}
export function editorObservation(doc: object, key: object, read: () => void): void {
	if (failed.has(doc)) return;
	const batch = active.get(doc);
	if (batch) batch.observers.set(key, read);
	else read();
}
export function editorLocalUpdate(
	doc: object,
	bytes: Uint8Array,
	sink: (bytes: Uint8Array) => void
): void {
	if (failed.has(doc)) return;
	const batch = active.get(doc);
	if (batch) batch.sinks.add(sink);
	else sink(bytes);
}
export function isEditorBatchActive(doc: object): boolean {
	return active.has(doc);
}
const mergePolicies = new WeakMap<UndoManager, { milliseconds: number; boundary: boolean }>();
export function registerEditorUndoPolicy(manager: UndoManager, milliseconds = 1000): void {
	if (!mergePolicies.has(manager)) mergePolicies.set(manager, { milliseconds, boundary: false });
}

export interface EditorAtomicOptions {
	groupIntent?: boolean;
	undoManager?: UndoManager;
	protectStorage?: boolean;
	/** Deterministic developer fault seam; no production caller supplies it. */ afterAction?: () => void;
}
export function atomicEditorWrite(
	doc: LoroDoc,
	action: () => void,
	options: EditorAtomicOptions = {}
): void {
	if (transientHolds.get(doc)?.size)
		throw new EditorPreflightError('This Page is waiting for a transfer receipt.');
	if (failed.has(doc) || pausedWrites.has(doc))
		throw new EditorWriteFailure('Editing is paused after an interrupted operation.', null);
	if (active.has(doc))
		throw new EditorPreflightError('Nested editor transactions are unsupported.');
	if (doc.getPendingTxnLength() !== 0)
		throw new EditorPreflightError('A different engine operation is still pending.');
	const batch: Batch = { from: doc.oplogVersion(), observers: new Map(), sinks: new Set() };
	const frontiers = doc.frontiers();
	let preimage: LoroDoc | undefined;
	let grouped = false;
	const explicit = options.groupIntent ?? options.protectStorage ?? false;
	const policy = options.undoManager ? mergePolicies.get(options.undoManager) : undefined;
	if (policy && (explicit || policy.boundary)) options.undoManager!.setMergeInterval(0);
	active.set(doc, batch);
	try {
		if (options.protectStorage) preimage = doc.fork();
		if (explicit) options.undoManager?.groupStart();
		grouped = explicit && !!options.undoManager;
		const result = action() as unknown;
		if (result && typeof (result as Promise<unknown>).then === 'function')
			throw new Error('Editor transaction must be synchronous.');
		options.afterAction?.();
		if (doc.getPendingTxnLength()) doc.commit({ origin: 'loroSyncPlugin' });
		if (grouped) {
			options.undoManager!.groupEnd();
			grouped = false;
		}
		// Export while still buffered, then let readers observe only the final valid document.
		const bytes = batch.sinks.size ? doc.export({ mode: 'update', from: batch.from }) : null;
		for (const read of batch.observers.values()) read();
		active.delete(doc);
		if (bytes) for (const sink of batch.sinks) sink(bytes);
		preimage?.free();
		preimage = undefined;
		if (policy) policy.boundary = explicit;
	} catch (error) {
		if (grouped) {
			try {
				options.undoManager!.groupEnd();
			} catch {
				/* Retired below. */
			}
		}
		// forkAt excludes every failed operation, while retaining all earlier local unsent history.
		preimage ??= doc.forkAt(frontiers);
		failed.add(doc);
		active.delete(doc);
		batch.observers.clear();
		batch.sinks.clear();
		const recover = recoveries.get(doc);
		if (recover) {
			const transferred = preimage;
			preimage = undefined;
			recover(transferred, error);
		} else {
			preimage.free();
			preimage = undefined;
		}
		throw new EditorWriteFailure(
			'Editing is paused. Your earlier text is preserved; undo history was reset.',
			error
		);
	} finally {
		active.delete(doc);
		if (policy) options.undoManager!.setMergeInterval(policy.milliseconds);
		batch.from.free();
		preimage?.free();
	}
}
