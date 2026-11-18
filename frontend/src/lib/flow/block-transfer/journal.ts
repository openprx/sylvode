/** Strict, account-isolated metadata. Never persist Page titles, body text or snapshots. */
import type { TransferCheckpoint } from './controller';
export interface TransferJournalEntry {
	actorId: string;
	sourcePageId: string;
	checkpoint: TransferCheckpoint;
}
export interface JournalStorage {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
	removeItem(key: string): void;
}
function obj(value: unknown, keys: string[]): value is Record<string, unknown> {
	return (
		!!value &&
		typeof value === 'object' &&
		!Array.isArray(value) &&
		Object.keys(value).every((key) => keys.includes(key))
	);
}
const str = (value: unknown, max = 4096): value is string =>
	typeof value === 'string' && value.length > 0 && value.length <= max;
function safe(value: unknown, actor: string): value is TransferJournalEntry {
	if (
		!obj(value, ['actorId', 'sourcePageId', 'checkpoint']) ||
		value.actorId !== actor ||
		!str(value.sourcePageId, 128)
	)
		return false;
	const checkpoint = value.checkpoint;
	if (
		!obj(checkpoint, ['state', 'action']) ||
		!['commit', 'undo'].includes(String(checkpoint.action))
	)
		return false;
	const state = checkpoint.state;
	if (
		!obj(state, [
			'phase',
			'review',
			'receipt',
			'detailsUnavailable',
			'error',
			'commitKey',
			'undoKey'
		]) ||
		!['prepared', 'committing', 'unknown', 'applied', 'undoing', 'undone', 'not_applied'].includes(
			String(state.phase)
		) ||
		!str(state.commitKey, 128) ||
		!str(state.undoKey, 128) ||
		typeof state.detailsUnavailable !== 'boolean' ||
		(state.error !== null &&
			![
				'review_changed',
				'dirty_pages',
				'confirmation_required',
				'request_failed',
				'undo_conflict',
				'receipt_mismatch'
			].includes(String(state.error)))
	)
		return false;
	const review = state.review;
	if (
		!obj(review, [
			'id',
			'kind',
			'sourcePageId',
			'targetPageId',
			'sourceFrontier',
			'targetFrontier',
			'digest',
			'expiresAt',
			'memberCount',
			'assetCount',
			'affectedShareCount',
			'assetsReady'
		]) ||
		review.sourcePageId !== value.sourcePageId ||
		!['move', 'copy'].includes(String(review.kind)) ||
		!['id', 'sourcePageId', 'targetPageId', 'sourceFrontier', 'targetFrontier', 'digest'].every(
			(key) => str(review[key])
		) ||
		!['expiresAt', 'memberCount', 'assetCount', 'affectedShareCount'].every(
			(key) => Number.isSafeInteger(review[key]) && Number(review[key]) >= 0
		) ||
		typeof review.assetsReady !== 'boolean'
	)
		return false;
	if (state.receipt !== null) {
		const receipt = state.receipt;
		if (
			!obj(receipt, [
				'id',
				'kind',
				'sourcePageId',
				'targetPageId',
				'sourcePostFrontier',
				'targetPostFrontier',
				'firstTargetBlockCid'
			]) ||
			receipt.id !== review.id ||
			receipt.kind !== review.kind ||
			receipt.sourcePageId !== review.sourcePageId ||
			receipt.targetPageId !== review.targetPageId ||
			!str(receipt.sourcePostFrontier) ||
			!str(receipt.targetPostFrontier) ||
			(receipt.firstTargetBlockCid !== null && !str(receipt.firstTargetBlockCid, 128))
		)
			return false;
	}
	return checkpoint.action !== 'undo' || state.receipt !== null;
}
export class BlockTransferJournal {
	constructor(
		private storage: JournalStorage,
		private actor: string
	) {}
	private key() {
		return `sv-block-transfers:v2:${this.actor}`;
	}
	list(): TransferJournalEntry[] {
		try {
			const value: unknown = JSON.parse(this.storage.getItem(this.key()) ?? '[]');
			return Array.isArray(value)
				? value.filter((entry) => safe(entry, this.actor)).slice(-32)
				: [];
		} catch {
			return [];
		}
	}
	put(entry: TransferJournalEntry): void {
		if (!safe(entry, this.actor)) throw new Error('invalid_transfer_journal');
		const entries = this.list().filter((old) => old.sourcePageId !== entry.sourcePageId);
		if (entries.length >= 32) throw new Error('transfer_journal_full');
		this.storage.setItem(this.key(), JSON.stringify([...entries, entry]));
	}
	remove(id: string): void {
		const entries = this.list().filter((entry) => entry.checkpoint.state.review!.id !== id);
		if (entries.length) this.storage.setItem(this.key(), JSON.stringify(entries));
		else this.storage.removeItem(this.key());
	}
}
