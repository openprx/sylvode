// State machine for the workspace Flow settings page (`/workspace/{id}/settings/flow`): reads and
// writes `GET|PUT /workspaces/{workspace_id}/features/flow` (`contracts/rest-api-v1.md` v0.4
// table; `surface-coverage-v1.md` UI consumer `adapter:CommandService` "workspace settings
// toggle").
//
// Rules this module owns, so they are unit-testable without a component:
// - each field is its own write intent with its own idempotency key; a request carries only the
//   one field that intent changes, and an empty change is refused before any request is made;
// - a retry of the same intent resends the same key (`crypto.randomUUID()` once per intent);
// - disabling Flow and changing `default_member_level` (a workspace-wide baseline change that
//   advances `authz_epoch`) are dangerous: `submit` refuses them until `confirm` was called for
//   that exact intent, which only a user action in the confirmation dialog does;
// - errors branch on `error_code` (and, for the legacy string-typed envelope that carries no
//   `error_code`, on the numeric business code) -- never on `message`.

import { authApi } from '$lib/api/auth';
import type { ApiResult } from '$lib/api/client';
import {
	FLOW_MEMBER_LEVELS,
	type FlowFeatureFlags,
	type FlowFeatureUpdate,
	type FlowMemberLevel,
	type SetFlowFeatureInput
} from '$lib/api/flow';
import { workspacesApi } from '$lib/api/workspaces';
import { flowFeatureStore, type FlowFeatureStore } from '$lib/stores/flow-feature';
import { FlowCommandService } from './command-service';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';

export { FLOW_MEMBER_LEVELS, type FlowMemberLevel };

export type FlowSettingsChange =
	| { readonly field: 'enabled'; readonly value: boolean }
	| { readonly field: 'default_member_level'; readonly value: FlowMemberLevel };

export interface FlowSettingsIntent {
	readonly change: FlowSettingsChange;
	readonly idempotencyKey: string;
	/** Disable and baseline changes need an explicit, user-produced confirmation. */
	readonly requiresConfirmation: boolean;
}

export type FlowSettingsLoadOutcome =
	| { readonly status: 'ready'; readonly feature: FlowFeatureFlags }
	| { readonly status: 'forbidden' }
	| { readonly status: 'failed'; readonly messageKey: string };

export type FlowSettingsSubmitOutcome =
	| {
			readonly status: 'saved';
			readonly feature: FlowFeatureFlags;
			readonly eventId: string | null;
	  }
	| { readonly status: 'confirmation_required' }
	| { readonly status: 'forbidden' }
	| { readonly status: 'failed'; readonly messageKey: string; readonly retryable: boolean };

export type FlowSettingsFailure =
	| { readonly kind: 'forbidden' }
	| { readonly kind: 'failed'; readonly messageKey: string; readonly retryable: boolean };

/** i18n keys for every `default_member_level` value (static, so the parity gate can see them). */
export const FLOW_MEMBER_LEVEL_KEYS: Readonly<Record<FlowMemberLevel, string>> = {
	full_access: 'flow.settings.level.full_access',
	edit: 'flow.settings.level.edit',
	comment: 'flow.settings.level.comment',
	view: 'flow.settings.level.view'
};

export class FlowSettingsChangeError extends Error {
	constructor(readonly reason: 'empty_change' | 'invalid_value' | 'stale_intent') {
		super(`flow settings change refused: ${reason}`);
		this.name = 'FlowSettingsChangeError';
	}
}

export function isFlowMemberLevel(value: unknown): value is FlowMemberLevel {
	return typeof value === 'string' && (FLOW_MEMBER_LEVELS as readonly string[]).includes(value);
}

/**
 * Builds the `PUT` body for one intent: exactly the one changed field plus the intent's key.
 * Throws `empty_change` for a missing change and `invalid_value` for a value outside the
 * contract's domain -- the server would reject both, and an empty request must never be sent.
 */
export function buildFlowFeatureRequest(
	change: FlowSettingsChange | null | undefined,
	idempotencyKey: string
): SetFlowFeatureInput {
	if (!change) throw new FlowSettingsChangeError('empty_change');
	if (change.field === 'enabled') {
		if (typeof change.value !== 'boolean') throw new FlowSettingsChangeError('invalid_value');
		return { enabled: change.value, idempotency_key: idempotencyKey };
	}
	if (change.field === 'default_member_level') {
		if (!isFlowMemberLevel(change.value)) throw new FlowSettingsChangeError('invalid_value');
		return { default_member_level: change.value, idempotency_key: idempotencyKey };
	}
	throw new FlowSettingsChangeError('empty_change');
}

/** Disabling Flow and any baseline change are confirmed; enabling is not. */
export function changeRequiresConfirmation(change: FlowSettingsChange): boolean {
	return change.field === 'default_member_level' || change.value === false;
}

/**
 * Classifies a non-success envelope. Reads `error_code` first; the legacy string-typed envelope
 * (`apps/api/src/error.rs::legacy_response`, no `error_code`) is classified by its numeric
 * business code, which `error-mapping-v1.md` fixes per class (`Forbidden`/403, `NotFound`/404).
 * `message` is never read. `not_found` lands on the same forbidden-safe state as `forbidden`
 * so the page does not reveal whether the workspace exists.
 */
export function classifyFlowSettingsError(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): FlowSettingsFailure {
	const flowError = flowErrorFromEnvelope(result);
	if (flowError) {
		if (flowError.code === 'forbidden' || flowError.code === 'not_found') {
			return { kind: 'forbidden' };
		}
		return {
			kind: 'failed',
			messageKey: flowErrorI18nKey(flowError),
			retryable: flowError.recoverable
		};
	}
	if (typeof result.error_code === 'string') {
		// A typed code this build does not know: fail closed, no retry advertised.
		return { kind: 'failed', messageKey: 'flow.settings.error.rejected', retryable: false };
	}
	switch (result.code) {
		case 403:
		case 404:
			return { kind: 'forbidden' };
		case 401:
			return { kind: 'failed', messageKey: 'flow.error.unauthenticated', retryable: false };
		case 409:
			return { kind: 'failed', messageKey: 'flow.settings.error.conflict', retryable: false };
		default:
			return result.code >= 500
				? { kind: 'failed', messageKey: 'flow.settings.error.unavailable', retryable: true }
				: { kind: 'failed', messageKey: 'flow.settings.error.rejected', retryable: false };
	}
}

function stripEventId(update: FlowFeatureUpdate): FlowFeatureFlags {
	return {
		flow_enabled: update.flow_enabled,
		default_member_level: update.default_member_level,
		authz_epoch: update.authz_epoch,
		updated_at: update.updated_at,
		updated_by: update.updated_by
	};
}

/** One key per write intent. `crypto.randomUUID` only exists in secure contexts, so a page served
 * over plain http on a LAN address falls back to a v4 UUID built from `getRandomValues`, which
 * is available everywhere. */
function newIdempotencyKey(): string {
	if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
	const bytes = crypto.getRandomValues(new Uint8Array(16));
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/**
 * Whether the signed-in user is an owner/admin of `workspaceId`. Used only to HIDE the form --
 * the server's `PUT` admin check is the real guard. Fails closed on any error.
 */
export async function resolveWorkspaceAdmin(workspaceId: string): Promise<boolean> {
	try {
		const [me, members] = await Promise.all([authApi.me(), workspacesApi.getMembers(workspaceId)]);
		const userId = me.code === 0 ? me.data?.user.id : undefined;
		if (!userId || members.code !== 0 || !members.data) return false;
		const role = members.data.items.find((member) => member.user_id === userId)?.role;
		return role === 'owner' || role === 'admin';
	} catch {
		return false;
	}
}

export interface FlowSettingsDeps {
	readonly commands: Pick<FlowCommandService, 'getFlowFeature' | 'setFlowFeature'>;
	readonly store: Pick<FlowFeatureStore, 'set'>;
	readonly isWorkspaceAdmin: (workspaceId: string) => Promise<boolean>;
	readonly newKey: () => string;
}

export class FlowSettingsController {
	private feature: FlowFeatureFlags | null = null;
	private pending: FlowSettingsIntent | null = null;
	private confirmed: FlowSettingsIntent | null = null;
	private readonly deps: FlowSettingsDeps;

	constructor(
		private readonly workspaceId: string,
		deps: Partial<FlowSettingsDeps> = {}
	) {
		this.deps = {
			commands: deps.commands ?? new FlowCommandService(),
			store: deps.store ?? flowFeatureStore,
			isWorkspaceAdmin: deps.isWorkspaceAdmin ?? resolveWorkspaceAdmin,
			newKey: deps.newKey ?? newIdempotencyKey
		};
	}

	get current(): FlowFeatureFlags | null {
		return this.feature;
	}

	get pendingIntent(): FlowSettingsIntent | null {
		return this.pending;
	}

	async load(): Promise<FlowSettingsLoadOutcome> {
		if (!(await this.deps.isWorkspaceAdmin(this.workspaceId))) return { status: 'forbidden' };
		const result = await this.deps.commands.getFlowFeature(this.workspaceId);
		if (result.code === 0 && result.data) {
			this.feature = result.data;
			this.deps.store.set(this.workspaceId, result.data.flow_enabled === true);
			return { status: 'ready', feature: result.data };
		}
		const failure = classifyFlowSettingsError(result);
		if (failure.kind === 'forbidden') return { status: 'forbidden' };
		return { status: 'failed', messageKey: 'flow.settings.error.loadFailed' };
	}

	/**
	 * Starts (or resumes) the write intent for one field. Returns `null` when the change would
	 * leave the setting as it already is. Proposing the change already pending returns that same
	 * intent -- same key -- so a double click or a retry never mints a second operation.
	 */
	propose(change: FlowSettingsChange | null | undefined): FlowSettingsIntent | null {
		buildFlowFeatureRequest(change, 'validation-only');
		const validChange = change as FlowSettingsChange;
		if (this.feature && this.matchesCurrent(validChange)) {
			this.cancel();
			return null;
		}
		const pending = this.pending;
		if (
			pending &&
			pending.change.field === validChange.field &&
			pending.change.value === validChange.value
		) {
			return pending;
		}
		this.confirmed = null;
		this.pending = {
			change: validChange,
			idempotencyKey: this.deps.newKey(),
			requiresConfirmation: changeRequiresConfirmation(validChange)
		};
		return this.pending;
	}

	/** Records the user's explicit confirmation for exactly this intent. */
	confirm(intent: FlowSettingsIntent): void {
		if (intent !== this.pending) throw new FlowSettingsChangeError('stale_intent');
		this.confirmed = intent;
	}

	cancel(): void {
		this.pending = null;
		this.confirmed = null;
	}

	async submit(intent: FlowSettingsIntent): Promise<FlowSettingsSubmitOutcome> {
		if (intent !== this.pending) throw new FlowSettingsChangeError('stale_intent');
		if (intent.requiresConfirmation && this.confirmed !== intent) {
			return { status: 'confirmation_required' };
		}
		const body = buildFlowFeatureRequest(intent.change, intent.idempotencyKey);
		const result = await this.deps.commands.setFlowFeature(this.workspaceId, body);
		if (result.code === 0 && result.data) {
			const feature = stripEventId(result.data);
			this.feature = feature;
			this.deps.store.set(this.workspaceId, feature.flow_enabled === true);
			if (this.pending === intent) this.cancel();
			return { status: 'saved', feature, eventId: result.data.event_id ?? null };
		}
		const failure = classifyFlowSettingsError(result);
		if (failure.kind === 'forbidden') {
			this.cancel();
			return { status: 'forbidden' };
		}
		// A retryable failure keeps the intent (and its key) for the retry; a permanent one ends it.
		if (!failure.retryable) this.cancel();
		return { status: 'failed', messageKey: failure.messageKey, retryable: failure.retryable };
	}

	private matchesCurrent(change: FlowSettingsChange): boolean {
		const feature = this.feature;
		if (!feature) return false;
		return change.field === 'enabled'
			? feature.flow_enabled === change.value
			: feature.default_member_level === change.value;
	}
}
