import type { FlowComment } from '../api/flow';
/** Mirrors the shared comment service's author/grade rules; the server remains authoritative. */
export function commentActions(
	comment: FlowComment,
	userId: string | null,
	permissions: { comment: boolean; edit: boolean; manage: boolean }
) {
	const author = !!userId && comment.author_id === userId;
	return {
		edit: author && permissions.comment,
		delete: (author && permissions.comment) || permissions.manage,
		resolve: (author && permissions.comment) || permissions.edit
	};
}
/** A reply never acquires a different current-body anchor or rebinds an orphan by CID. */
export function commentCreationAnchor(
	anchor: string | null,
	replyTo: string | null,
	comments: readonly Pick<FlowComment, 'id' | 'block_id' | 'anchor_status'>[]
): string | null {
	if (!replyTo) return anchor;
	const parent = comments.find((comment) => comment.id === replyTo);
	return !parent || parent.anchor_status === 'orphaned' ? null : (parent.block_id ?? null);
}
/** Component-owned retry intents, never shared between objects/accounts or stored with tokens. */
export class FlowMutationIntents {
	private readonly entries = new Map<string, { signature: string; key: string }>();
	constructor(private readonly newKey: () => string = () => crypto.randomUUID()) {}
	key(operation: string, payload: unknown): string {
		const signature = JSON.stringify(payload);
		const previous = this.entries.get(operation);
		if (previous?.signature === signature) return previous.key;
		const key = this.newKey();
		this.entries.set(operation, { signature, key });
		if (this.entries.size > 64) {
			const oldest = this.entries.keys().next().value;
			if (oldest !== undefined) this.entries.delete(oldest);
		}
		return key;
	}
	complete(operation: string): void {
		this.entries.delete(operation);
	}
}
