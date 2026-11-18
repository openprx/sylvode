/** Separate recipient vocabulary: it cannot represent a Page ID, CID, head or private title. */
import type { PageInlineSpan } from '../inline-runs';
import { safeEditorHref } from '../editor-schema';
export type SharedTextKind = 'paragraph' | 'bullet' | 'quote';
type Marked = { text: string; spans: PageInlineSpan[] };
export type SharedContent =
	| ({ kind: SharedTextKind } & Marked)
	| ({ kind: 'heading'; level: number } & Marked)
	| ({ kind: 'ordered'; order: number } & Marked)
	| ({ kind: 'todo'; checked: boolean } & Marked)
	| ({ kind: 'toggle'; collapsed: boolean } & Marked)
	| { kind: 'code'; text: string }
	| { kind: 'divider' | 'child_placeholder' }
	| { kind: 'image'; asset_alias: string; alt: string; caption: string }
	| { kind: 'attachment'; asset_alias: string; name: string; caption: string; mime: string };
export interface SharedMember {
	member_id: string;
	parent_member_id: string | null;
	order: number;
	content: SharedContent;
}
export interface SharedProjection {
	share_id: string;
	revision: number;
	members: SharedMember[];
}
export interface SharedDisplayContext {
	placement_id: string;
	label: 'Shared block';
	workspace_id: string;
	workspace_name: string;
	project_id: string | null;
	project_name: string | null;
}
export interface SharedRead {
	projection: SharedProjection;
	effective_permission: 'view' | 'edit';
	display_context?: SharedDisplayContext;
	receipt?: { accepted_revision: number; replayed: boolean };
}
export type SharedCommand =
	| { type: 'replace_text'; member_id: string; text: string; spans: PageInlineSpan[] }
	| { type: 'set_marks'; member_id: string; edits: (PageInlineSpan & { clear?: boolean })[] }
	| { type: 'set_checked'; member_id: string; checked: boolean }
	| { type: 'set_collapsed'; member_id: string; collapsed: boolean };
export interface SharedPatch {
	expected_revision: number;
	commands: SharedCommand[];
}
export interface SharedAudience {
	workspace_id: string;
	project_id?: string | null;
	audience: 'workspace' | 'users';
	recipient_user_ids?: string[];
	permission: 'view' | 'edit';
}
export interface CreateBlockShare {
	expected_frontier: string;
	root_block_cid: string;
	selection: 'block' | 'current_subtree';
	audiences: [SharedAudience];
}
export interface ManagedBlockShare {
	root_block_cid: string;
	selection: 'block' | 'current_subtree';
	share_id: string;
	state: 'active' | 'suspended' | 'revoked' | 'target_removed';
	revision: number;
	member_count: number;
	audiences: [SharedAudience];
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
function object(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value))
		throw new Error('invalid_shared_projection');
	return value as Record<string, unknown>;
}
function keys(value: Record<string, unknown>, allowed: string[]): void {
	if (Object.keys(value).some((key) => !allowed.includes(key)))
		throw new Error('invalid_shared_projection');
}
function integer(value: unknown, minimum = 0): value is number {
	return Number.isSafeInteger(value) && Number(value) >= minimum;
}
function id(value: unknown): value is string {
	return typeof value === 'string' && uuid.test(value);
}
export function sharedSpansValid(text: string, spans: unknown): spans is PageInlineSpan[] {
	if (!Array.isArray(spans)) return false;
	const boundaries = new Set<number>([0]);
	let length = 0;
	for (const char of text) {
		length += new TextEncoder().encode(char).length;
		boundaries.add(length);
	}
	try {
		return spans.every((raw) => {
			const span = object(raw);
			keys(span, ['start', 'end', 'mark', 'href']);
			return (
				integer(span.start) &&
				integer(span.end) &&
				Number(span.start) < Number(span.end) &&
				boundaries.has(Number(span.start)) &&
				boundaries.has(Number(span.end)) &&
				['bold', 'italic', 'strike', 'code', 'link'].includes(String(span.mark)) &&
				(span.mark === 'link'
					? typeof span.href === 'string' && safeEditorHref(span.href)
					: span.href === undefined)
			);
		});
	} catch {
		return false;
	}
}
/** Fail closed on a wrong full-Page response instead of accepting its private extra fields. */
export function parseSharedContext(value: unknown): SharedDisplayContext {
	const context = object(value);
	keys(context, [
		'placement_id',
		'label',
		'workspace_id',
		'workspace_name',
		'project_id',
		'project_name'
	]);
	if (
		!id(context.placement_id) ||
		context.label !== 'Shared block' ||
		!id(context.workspace_id) ||
		typeof context.workspace_name !== 'string' ||
		(context.project_id !== null && !id(context.project_id)) ||
		(context.project_name !== null && typeof context.project_name !== 'string') ||
		(context.project_id === null) !== (context.project_name === null)
	)
		throw new Error('invalid_shared_context');
	return JSON.parse(JSON.stringify(value)) as SharedDisplayContext;
}

export function parseSharedRead(value: unknown, expectedShareId?: string): SharedRead {
	const read = object(value);
	keys(read, ['projection', 'effective_permission', 'display_context', 'receipt']);
	if (read.display_context !== undefined) parseSharedContext(read.display_context);
	if (read.receipt !== undefined) {
		const receipt = object(read.receipt);
		keys(receipt, ['accepted_revision', 'replayed']);
		if (!integer(receipt.accepted_revision) || typeof receipt.replayed !== 'boolean')
			throw new Error('invalid_shared_receipt');
	}
	if (read.effective_permission !== 'view' && read.effective_permission !== 'edit')
		throw new Error('invalid_shared_permission');
	const projection = object(read.projection);
	keys(projection, ['share_id', 'revision', 'members']);
	if (
		!id(projection.share_id) ||
		(expectedShareId && projection.share_id !== expectedShareId) ||
		!integer(projection.revision) ||
		!Array.isArray(projection.members) ||
		!projection.members.length ||
		projection.members.length > 256
	)
		throw new Error('invalid_shared_projection');
	const parents = new Map<string, string | null>();
	for (const raw of projection.members) {
		const member = object(raw);
		keys(member, ['member_id', 'parent_member_id', 'order', 'content']);
		if (
			!id(member.member_id) ||
			parents.has(member.member_id) ||
			(member.parent_member_id !== null && !id(member.parent_member_id)) ||
			!integer(member.order)
		)
			throw new Error('invalid_shared_member');
		parents.set(member.member_id, member.parent_member_id as string | null);
		const content = object(member.content),
			kind = String(content.kind);
		const extra: Record<string, string[]> = {
			heading: ['level'],
			ordered: ['order'],
			todo: ['checked'],
			toggle: ['collapsed']
		};
		if (['paragraph', 'heading', 'bullet', 'ordered', 'todo', 'quote', 'toggle'].includes(kind)) {
			keys(content, ['kind', 'text', 'spans', ...(extra[kind] ?? [])]);
			if (typeof content.text !== 'string' || !sharedSpansValid(content.text, content.spans))
				throw new Error('invalid_shared_text');
			if (kind === 'heading' && (!integer(content.level, 1) || Number(content.level) > 3))
				throw new Error('invalid_shared_heading');
			if (kind === 'ordered' && !integer(content.order, 1)) throw new Error('invalid_shared_order');
			if (
				(kind === 'todo' && typeof content.checked !== 'boolean') ||
				(kind === 'toggle' && typeof content.collapsed !== 'boolean')
			)
				throw new Error('invalid_shared_state');
		} else if (kind === 'code') {
			keys(content, ['kind', 'text']);
			if (typeof content.text !== 'string') throw new Error('invalid_shared_text');
		} else if (kind === 'divider' || kind === 'child_placeholder') keys(content, ['kind']);
		else if (kind === 'image') {
			keys(content, ['kind', 'asset_alias', 'alt', 'caption']);
			if (
				!id(content.asset_alias) ||
				typeof content.alt !== 'string' ||
				typeof content.caption !== 'string'
			)
				throw new Error('invalid_shared_asset');
		} else if (kind === 'attachment') {
			keys(content, ['kind', 'asset_alias', 'name', 'caption', 'mime']);
			if (
				!id(content.asset_alias) ||
				typeof content.name !== 'string' ||
				typeof content.caption !== 'string' ||
				typeof content.mime !== 'string'
			)
				throw new Error('invalid_shared_asset');
		} else throw new Error('unsupported_shared_content');
	}
	for (const [member, parent] of parents) {
		const seen = new Set<string>([member]);
		let cursor = parent,
			depth = 0;
		while (cursor !== null) {
			if (seen.has(cursor) || !parents.has(cursor) || ++depth > 7)
				throw new Error('invalid_shared_hierarchy');
			seen.add(cursor);
			cursor = parents.get(cursor)!;
		}
	}
	return JSON.parse(JSON.stringify(value)) as SharedRead;
}

/** Fixed-membership preorder only; no recursive source fetch can enter this projection. */
export function sharedProjectionRows(
	read: SharedRead
): { member: SharedMember; depth: number; listNumber?: number }[] {
	const members = parseSharedRead(read).projection.members;
	const result: { member: SharedMember; depth: number; listNumber?: number }[] = [];
	const walk = (parent: string | null, depth: number) => {
		const children = members
			.filter((member) => member.parent_member_id === parent)
			.sort((a, b) => a.order - b.order || a.member_id.localeCompare(b.member_id));
		let next: number | null = null;
		for (const member of children) {
			const listNumber: number | undefined =
				member.content.kind === 'ordered' ? (next ?? member.content.order) : undefined;
			next = listNumber === undefined ? null : listNumber + 1;
			result.push({ member, depth, ...(listNumber === undefined ? {} : { listNumber }) });
			walk(member.member_id, depth + 1);
		}
	};
	walk(null, 0);
	return result;
}

export interface SharedDiscovery {
	share_id: string;
	placement_id: string;
	revision: number;
	effective_permission: 'view' | 'edit';
	display_context: SharedDisplayContext;
}
export interface SharedTargetContext {
	workspace_id: string;
	project_id?: string | null;
}
export function parseSharedDiscovery(value: unknown): SharedDiscovery[] {
	if (!Array.isArray(value)) throw new Error('invalid_shared_discovery');
	return value.map((raw) => {
		const entry = object(raw);
		keys(entry, [
			'share_id',
			'placement_id',
			'revision',
			'effective_permission',
			'display_context'
		]);
		if (!id(entry.share_id) || !id(entry.placement_id) || !integer(entry.revision))
			throw new Error('invalid_shared_discovery');
		if (entry.effective_permission !== 'view' && entry.effective_permission !== 'edit')
			throw new Error('invalid_shared_permission');
		parseSharedContext(entry.display_context);
		return JSON.parse(JSON.stringify(raw)) as SharedDiscovery;
	});
}
export interface SharedPlacementManagement {
	share_id: string;
	placement_id: string;
	state: 'active' | 'suspended' | 'revoked' | 'target_removed';
	can_read: boolean;
	display_context: SharedDisplayContext;
}
export function parseSharedPlacementManagement(value: unknown): SharedPlacementManagement[] {
	if (!Array.isArray(value)) throw new Error('invalid_shared_placements');
	return value.map((raw) => {
		const row = object(raw);
		keys(row, ['share_id', 'placement_id', 'state', 'can_read', 'display_context']);
		if (
			!id(row.share_id) ||
			!id(row.placement_id) ||
			!['active', 'suspended', 'revoked', 'target_removed'].includes(String(row.state)) ||
			typeof row.can_read !== 'boolean'
		)
			throw new Error('invalid_shared_placement');
		const context = parseSharedContext(row.display_context);
		if (context.placement_id !== row.placement_id) throw new Error('invalid_shared_placement');
		return JSON.parse(JSON.stringify(raw)) as SharedPlacementManagement;
	});
}
