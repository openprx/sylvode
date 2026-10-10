// Share panel state machine (`contracts/ui-surface-v1.md` "职责边界" Share panel, `ADR-0012`).
//
// Rules this module owns, so they are unit-testable without a component:
// - the roster is read through `ObjectRepository.getGrants`; every write goes through
//   `CommandService.setGrants` / `setInheritance`;
// - a caller without `full_access` sees only its own effective level and the inheritance flag:
//   the roster fields are dropped even if a response carried them, no write is ever sent, and the
//   member directory is never requested;
// - any change that can LOWER someone's permission (removing a grant, lowering a level, clearing
//   the roster, breaking inheritance) is first sent with `dry_run:true` and its
//   `permission_changes` are shown; only an explicit confirm sends the real request. A change
//   that only adds or raises grants is sent directly;
// - when the caller would lose its own `full_access` (`policy_rejected` with
//   `details.action = "authz_self_lockout"`, `apps/api/src/flow/grants.rs`), the real request
//   needs a user-produced acknowledgement; `confirm_self_lockout` is never pre-filled;
// - one idempotency key per intent: the dry run, the confirm and every retry of that intent send
//   the same key (a dry run does not consume it, `rest-api-v1.md` "不消费 idempotency_key");
// - a preview is never treated as authorization: after a commit the panel shows the commit
//   response's `permission_changes` and re-reads the roster from the server.
//
// Errors are classified only by `error_code` (and the typed `details.action` discriminator);
// `message` is never read.

import type { ApiResult } from '$lib/api/client';
import type {
	FlowEffectiveLevel,
	FlowGrantInput,
	FlowGrantLevel,
	FlowGrantsView,
	FlowPermissionChanges,
	FlowPrincipalKind,
	SetFlowGrantsInput,
	SetFlowInheritanceInput
} from '$lib/api/flow';
import { botsApi, type Bot } from '$lib/api/bots';
import { membersApi } from '$lib/api/members';
import { FlowCommandService } from './command-service';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';
import { FlowObjectRepository } from './object-repository';
import { newIdempotencyKey } from './package-export';

export const FLOW_GRANT_LEVELS = [
	'full_access',
	'edit',
	'comment',
	'view'
] as const satisfies readonly FlowGrantLevel[];

const LEVEL_RANK: Readonly<Record<FlowEffectiveLevel, number>> = {
	none: 0,
	view: 1,
	comment: 2,
	edit: 3,
	full_access: 4
};

/** i18n keys for every level a summary can carry (static, so tests can check them). */
export const SHARE_LEVEL_KEYS: Readonly<Record<FlowEffectiveLevel, string>> = {
	full_access: 'flow.share.level.full_access',
	edit: 'flow.share.level.edit',
	comment: 'flow.share.level.comment',
	view: 'flow.share.level.view',
	none: 'flow.share.level.none'
};

export const SHARE_PRINCIPAL_KIND_KEYS: Readonly<Record<FlowPrincipalKind, string>> = {
	user: 'flow.share.principal.user',
	bot: 'flow.share.principal.bot'
};

export function shareLevelKey(level: string): string {
	return (SHARE_LEVEL_KEYS as Record<string, string>)[level] ?? 'flow.share.level.unknown';
}

export function levelRank(level: string): number {
	return (LEVEL_RANK as Record<string, number>)[level] ?? 0;
}

function principalKey(kind: string, id: string): string {
	return `${kind}:${id}`;
}

/** True when `after` takes anything away from `before`: a principal disappears, or keeps a row at
 * a lower level. Adding principals and raising levels are not downgrades. */
export function isGrantDowngrade(
	before: readonly FlowGrantInput[],
	after: readonly FlowGrantInput[]
): boolean {
	const next = new Map(
		after.map((grant) => [principalKey(grant.principal_kind, grant.principal_id), grant])
	);
	return before.some((grant) => {
		const kept = next.get(principalKey(grant.principal_kind, grant.principal_id));
		return !kept || levelRank(kept.level) < levelRank(grant.level);
	});
}

export type ShareChange =
	| {
			readonly kind: 'add';
			readonly principal_kind: FlowPrincipalKind;
			readonly principal_id: string;
			readonly level: FlowGrantLevel;
	  }
	| {
			readonly kind: 'set_level';
			readonly principal_kind: FlowPrincipalKind;
			readonly principal_id: string;
			readonly level: FlowGrantLevel;
	  }
	| {
			readonly kind: 'remove';
			readonly principal_kind: FlowPrincipalKind;
			readonly principal_id: string;
	  }
	| { readonly kind: 'clear' }
	| { readonly kind: 'inheritance'; readonly inherit_from_parent: boolean };

function rosterOf(view: FlowGrantsView): FlowGrantInput[] {
	return view.items.map((item) => ({
		principal_kind: item.principal_kind,
		principal_id: item.principal_id,
		level: item.level
	}));
}

/** The complete replacement roster a grants change sends (`PUT .../grants` replaces the table). */
export function rosterAfter(view: FlowGrantsView, change: ShareChange): FlowGrantInput[] {
	const roster = rosterOf(view);
	switch (change.kind) {
		case 'add':
		case 'set_level': {
			const key = principalKey(change.principal_kind, change.principal_id);
			const rest = roster.filter(
				(grant) => principalKey(grant.principal_kind, grant.principal_id) !== key
			);
			return [
				...rest,
				{
					principal_kind: change.principal_kind,
					principal_id: change.principal_id,
					level: change.level
				}
			];
		}
		case 'remove':
			return roster.filter(
				(grant) =>
					principalKey(grant.principal_kind, grant.principal_id) !==
					principalKey(change.principal_kind, change.principal_id)
			);
		case 'clear':
			return [];
		case 'inheritance':
			return roster;
	}
}

/** Whether `change` can lower anyone's permission and therefore needs the dry-run preview. */
export function changeLowersPermissions(view: FlowGrantsView, change: ShareChange): boolean {
	if (change.kind === 'inheritance') {
		// Breaking inheritance cuts the workspace baseline and every ancestor grant.
		return view.inherit_from_parent && !change.inherit_from_parent;
	}
	return isGrantDowngrade(rosterOf(view), rosterAfter(view, change));
}

export interface ShareFailure {
	readonly messageKey: string;
	readonly retryable: boolean;
}

/** Classifies a rejected Share envelope by `error_code`; a legacy envelope by its numeric code. */
export function classifyShareFailure(envelope: {
	code: number;
	error_code?: string | null;
	details?: unknown;
}): ShareFailure {
	const flowError = flowErrorFromEnvelope(envelope);
	if (flowError)
		return { messageKey: flowErrorI18nKey(flowError), retryable: flowError.recoverable };
	if (typeof envelope.error_code === 'string') {
		return { messageKey: 'flow.share.error.rejected', retryable: false };
	}
	switch (envelope.code) {
		case 401:
			return { messageKey: 'flow.error.unauthenticated', retryable: true };
		case 403:
			return { messageKey: 'flow.error.forbidden', retryable: false };
		case 404:
			return { messageKey: 'flow.error.not_found', retryable: false };
		default:
			return envelope.code >= 500
				? { messageKey: 'flow.share.error.unavailable', retryable: true }
				: { messageKey: 'flow.share.error.rejected', retryable: false };
	}
}

/** `policy_rejected` with `details.action = "authz_self_lockout"`: the caller would lose its own
 * `full_access`. Returns the caller summary the server put in `details`, or `null` when the
 * envelope is anything else. */
export function selfLockoutDetails(envelope: {
	code: number;
	error_code?: string | null;
	details?: unknown;
}): FlowPermissionChanges['caller'] | null {
	if (envelope.error_code !== 'policy_rejected') return null;
	const details = envelope.details;
	if (typeof details !== 'object' || details === null) return null;
	if ((details as { action?: unknown }).action !== 'authz_self_lockout') return null;
	const caller = (details as { caller?: unknown }).caller;
	if (typeof caller !== 'object' || caller === null) {
		return { before_level: 'full_access', after_level: 'none', loses_full_access: true };
	}
	const { before_level, after_level } = caller as { before_level?: unknown; after_level?: unknown };
	return {
		before_level: (typeof before_level === 'string'
			? before_level
			: 'full_access') as FlowEffectiveLevel,
		after_level: (typeof after_level === 'string' ? after_level : 'none') as FlowEffectiveLevel,
		loses_full_access: true
	};
}

export interface ShareDirectoryEntry {
	readonly principal_kind: FlowPrincipalKind;
	readonly principal_id: string;
	readonly label: string;
}

/** Where the directory comes from: the workspace members and the workspace bots. */
export interface ShareDirectorySources {
	readonly members: typeof membersApi.list;
	readonly bots: (workspaceId: string) => Promise<ApiResult<Bot[]>>;
}

const DEFAULT_DIRECTORY_SOURCES: ShareDirectorySources = {
	members: (workspaceId) => membersApi.list(workspaceId),
	bots: (workspaceId) => botsApi.list(workspaceId)
};

/**
 * Who a grant can name, for labels and the add picker: the workspace's human members
 * (`principal_kind: "user"`, the user id) and its active bots (`principal_kind: "bot"`, the
 * `workspace_bots` id the grants store). Each source fails soft to nothing on its own, so a
 * caller who may not list bots still gets the members.
 */
export async function listShareDirectory(
	workspaceId: string,
	sources: ShareDirectorySources = DEFAULT_DIRECTORY_SOURCES
): Promise<ShareDirectoryEntry[]> {
	const [members, bots] = await Promise.all([
		sources
			.members(workspaceId)
			.then((result) =>
				result.code === 0 && result.data
					? result.data.items
							.filter(
								(member) => member.entity_type === undefined || member.entity_type === 'human'
							)
							.map((member) => ({
								principal_kind: 'user' as const,
								principal_id: member.user_id,
								label: member.name || member.email || member.user_id
							}))
					: []
			)
			.catch((): ShareDirectoryEntry[] => []),
		sources
			.bots(workspaceId)
			.then((result) =>
				result.code === 0 && Array.isArray(result.data)
					? result.data
							.filter((bot) => bot.is_active)
							.map((bot) => ({
								principal_kind: 'bot' as const,
								principal_id: bot.id,
								label: bot.name || bot.id
							}))
					: []
			)
			.catch((): ShareDirectoryEntry[] => [])
	]);
	return [...members, ...bots];
}

export type ShareLoadState =
	| { readonly status: 'loading' }
	| { readonly status: 'failed'; readonly failure: ShareFailure }
	| { readonly status: 'ready'; readonly view: FlowGrantsView; readonly canManage: boolean };

export type SharePendingPhase = 'previewing' | 'confirm' | 'submitting' | 'failed';

export interface SharePending {
	readonly change: ShareChange;
	/** The replacement roster a grants change sends, fixed when the intent starts so the dry run,
	 * the confirm and every retry send the same body under the same key. Not sent for an
	 * inheritance change (no `initial_grants`: the explicit grants stay untouched). */
	readonly roster: FlowGrantInput[];
	readonly key: string;
	readonly phase: SharePendingPhase;
	/** `permission_changes` of the dry run. Display only; never treated as authorization. */
	readonly preview: FlowPermissionChanges | null;
	/** The caller's own change, from the preview or the self-lockout rejection. */
	readonly caller: FlowPermissionChanges['caller'] | null;
	readonly selfLockout: boolean;
	readonly acknowledged: boolean;
	readonly failure: ShareFailure | null;
}

export interface ShareReceipt {
	readonly eventId: string | null;
	readonly changes: FlowPermissionChanges;
}

export interface ShareSnapshot {
	readonly load: ShareLoadState;
	readonly pending: SharePending | null;
	readonly receipt: ShareReceipt | null;
	readonly directory: readonly ShareDirectoryEntry[];
}

export type ShareOutcome =
	| { readonly status: 'refused' }
	| { readonly status: 'previewed' }
	| { readonly status: 'committed' }
	| { readonly status: 'failed'; readonly failure: ShareFailure };

export interface SharePanelDeps {
	readonly reads: Pick<FlowObjectRepository, 'getGrants'>;
	readonly commands: Pick<FlowCommandService, 'setGrants' | 'setInheritance'>;
	readonly directory: () => Promise<ShareDirectoryEntry[]>;
	readonly newKey: () => string;
}

type PermissionResponse = {
	applied: boolean;
	event_id: string | null;
	permission_changes: FlowPermissionChanges;
};

export class FlowSharePanelController {
	private readonly deps: SharePanelDeps;
	private load_: ShareLoadState = { status: 'loading' };
	private pending_: SharePending | null = null;
	private receipt_: ShareReceipt | null = null;
	private directory_: ShareDirectoryEntry[] = [];
	private generation = 0;

	constructor(
		private readonly objectId: string,
		workspaceId: string,
		private readonly onChange: (snapshot: ShareSnapshot) => void = () => {},
		deps: Partial<SharePanelDeps> = {}
	) {
		this.deps = {
			reads: deps.reads ?? new FlowObjectRepository(),
			commands: deps.commands ?? new FlowCommandService(),
			directory: deps.directory ?? (() => listShareDirectory(workspaceId)),
			newKey: deps.newKey ?? newIdempotencyKey
		};
	}

	get snapshot(): ShareSnapshot {
		return {
			load: this.load_,
			pending: this.pending_,
			receipt: this.receipt_,
			directory: this.directory_
		};
	}

	/** Reads the share state. Without `full_access` the roster is dropped and no directory read
	 * happens. */
	async load(): Promise<ShareLoadState> {
		const generation = ++this.generation;
		this.load_ = { status: 'loading' };
		this.emit();
		const result = await this.deps.reads.getGrants(this.objectId);
		if (generation !== this.generation) return this.load_;
		if (result.code !== 0 || !result.data) {
			this.load_ = { status: 'failed', failure: classifyShareFailure(result) };
			this.emit();
			return this.load_;
		}
		const canManage = result.data.effective_level === 'full_access';
		const view: FlowGrantsView = canManage
			? result.data
			: {
					items: [],
					inherited: [],
					inherit_from_parent: result.data.inherit_from_parent,
					effective_level: result.data.effective_level
				};
		this.load_ = { status: 'ready', view, canManage };
		this.emit();
		if (canManage && this.directory_.length === 0) {
			const directory = await this.deps.directory();
			if (generation === this.generation) {
				this.directory_ = directory;
				this.emit();
			}
		}
		return this.load_;
	}

	/** Starts a write intent. Downgrades stop at the preview (`previewed`); everything else is
	 * committed directly. */
	async propose(change: ShareChange): Promise<ShareOutcome> {
		if (this.load_.status !== 'ready' || !this.load_.canManage) return { status: 'refused' };
		if (this.pending_ && this.pending_.phase !== 'failed') return { status: 'refused' };
		const view = this.load_.view;
		const pending: SharePending = {
			change,
			roster: rosterAfter(view, change),
			key: this.deps.newKey(),
			phase: 'previewing',
			preview: null,
			caller: null,
			selfLockout: false,
			acknowledged: false,
			failure: null
		};
		this.receipt_ = null;
		if (!changeLowersPermissions(view, change)) {
			this.pending_ = { ...pending, phase: 'submitting' };
			this.emit();
			return this.commit(false);
		}
		this.pending_ = pending;
		this.emit();
		return this.preview(false);
	}

	/** The user's self-lockout acknowledgement. Ticking it re-runs the dry run with
	 * `confirm_self_lockout:true` so the full consequence list can be shown before the commit. */
	async acknowledgeSelfLockout(value: boolean): Promise<ShareOutcome> {
		const pending = this.pending_;
		if (!pending || !pending.selfLockout || pending.phase !== 'confirm')
			return { status: 'refused' };
		this.pending_ = { ...pending, acknowledged: value };
		this.emit();
		if (!value) return { status: 'previewed' };
		this.pending_ = { ...this.pending_, phase: 'previewing' };
		this.emit();
		return this.preview(true);
	}

	/** Sends the real request for the pending intent, with the intent's key. */
	async confirm(): Promise<ShareOutcome> {
		const pending = this.pending_;
		if (!pending || (pending.phase !== 'confirm' && pending.phase !== 'failed')) {
			return { status: 'refused' };
		}
		if (pending.selfLockout && !pending.acknowledged) return { status: 'refused' };
		this.pending_ = { ...pending, phase: 'submitting', failure: null };
		this.emit();
		return this.commit(pending.selfLockout && pending.acknowledged);
	}

	cancel(): void {
		this.pending_ = null;
		this.emit();
	}

	private request(
		pending: SharePending,
		options: { dryRun: boolean; confirmSelfLockout: boolean }
	): Promise<ApiResult<PermissionResponse>> {
		const change = pending.change;
		if (change.kind === 'inheritance') {
			const body: SetFlowInheritanceInput = {
				inherit_from_parent: change.inherit_from_parent,
				confirm_self_lockout: options.confirmSelfLockout,
				dry_run: options.dryRun,
				idempotency_key: pending.key
			};
			return this.deps.commands.setInheritance(this.objectId, body);
		}
		const body: SetFlowGrantsInput = {
			grants: pending.roster,
			confirm_self_lockout: options.confirmSelfLockout,
			dry_run: options.dryRun,
			idempotency_key: pending.key
		};
		return this.deps.commands.setGrants(this.objectId, body);
	}

	private async preview(confirmSelfLockout: boolean): Promise<ShareOutcome> {
		const pending = this.pending_;
		if (!pending) return { status: 'refused' };
		const result = await this.request(pending, { dryRun: true, confirmSelfLockout });
		if (this.pending_?.key !== pending.key) return { status: 'refused' };
		if (result.code === 0 && result.data) {
			const changes = result.data.permission_changes;
			this.pending_ = {
				...this.pending_,
				phase: 'confirm',
				preview: changes,
				caller: changes.caller,
				selfLockout: this.pending_.selfLockout || changes.caller.loses_full_access
			};
			this.emit();
			return { status: 'previewed' };
		}
		const lockout = selfLockoutDetails(result);
		if (lockout) {
			this.pending_ = {
				...this.pending_,
				phase: 'confirm',
				preview: null,
				caller: lockout,
				selfLockout: true,
				acknowledged: false
			};
			this.emit();
			return { status: 'previewed' };
		}
		const failure = classifyShareFailure(result);
		this.pending_ = { ...this.pending_, phase: 'failed', failure };
		this.emit();
		return { status: 'failed', failure };
	}

	private async commit(confirmSelfLockout: boolean): Promise<ShareOutcome> {
		const pending = this.pending_;
		if (!pending) return { status: 'refused' };
		const result = await this.request(pending, { dryRun: false, confirmSelfLockout });
		if (this.pending_?.key !== pending.key) return { status: 'refused' };
		if (result.code === 0 && result.data) {
			// The server's answer is the only record of what changed; the preview is dropped.
			this.receipt_ = { eventId: result.data.event_id, changes: result.data.permission_changes };
			this.pending_ = null;
			this.emit();
			await this.load();
			return { status: 'committed' };
		}
		const lockout = selfLockoutDetails(result);
		if (lockout) {
			// The post-state changed since the preview: back to confirm, same key, no acknowledgement.
			this.pending_ = {
				...this.pending_,
				phase: 'confirm',
				preview: null,
				caller: lockout,
				selfLockout: true,
				acknowledged: false
			};
			this.emit();
			return { status: 'previewed' };
		}
		const failure = classifyShareFailure(result);
		this.pending_ = { ...this.pending_, phase: 'failed', failure };
		this.emit();
		return { status: 'failed', failure };
	}

	private emit(): void {
		this.onChange(this.snapshot);
	}
}
