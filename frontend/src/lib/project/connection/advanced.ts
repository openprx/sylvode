/**
 * The "Advanced" fields as one draft: the execution fields (channel, what happens on completion,
 * the failure label) and the governance fields that remain on a connection. Pure functions turn a
 * connection into a draft, a draft into the request fields, and compare the two for an update.
 */
import type { ConnectInput, GovernanceInput, UpdateInput } from '#lib/api/bot-connection.js';
import {
	ERRORS,
	advancedValid,
	parseReasonLength,
	validateAdvanced,
	type AdvancedErrors
} from './validate';
import type { Channel, Connection, Level } from './wire';

export type OnComplete = 'auto' | 'none' | 'state';

export interface DomainRow {
	domain: string | null;
	level: Level | null;
}

export interface Draft {
	channel: Channel;
	onComplete: OnComplete;
	stateKey: string | null;
	failLabel: string | null;
	level: Level | null;
	veto: boolean;
	reasonLength: string;
	domains: DomainRow[];
}

/** What a new connection starts with: the server's defaults. */
export function defaultDraft(): Draft {
	return {
		channel: 'tmux',
		onComplete: 'auto',
		stateKey: null,
		failLabel: null,
		level: 'voter',
		veto: false,
		reasonLength: '50',
		domains: []
	};
}

export function draftOf(connection: Connection): Draft {
	const complete = connection.execution.on_complete_state;
	const stateKey = complete.startsWith('state:') ? complete.slice('state:'.length) : null;
	return {
		channel: connection.execution.channel,
		onComplete: stateKey !== null ? 'state' : complete === 'none' ? 'none' : 'auto',
		stateKey,
		failLabel: connection.execution.on_fail_label_id,
		level: connection.governance.max_domain_level,
		veto: connection.governance.can_veto_human_consensus,
		reasonLength: String(connection.governance.reason_min_length),
		domains: Object.entries(connection.governance.domain_overrides).map(([domain, level]) => ({
			domain,
			level
		}))
	};
}

/** `auto`, `none` or `state:<key>`, as the server stores it. */
export function onCompleteValue(draft: Draft): string {
	return draft.onComplete === 'state' && draft.stateKey !== null
		? `state:${draft.stateKey}`
		: draft.onComplete === 'state'
			? 'auto'
			: draft.onComplete;
}

export function governanceOf(draft: Draft): GovernanceInput {
	const overrides: Record<string, Level> = {};
	for (const row of draft.domains)
		if (row.domain !== null && row.level !== null) overrides[row.domain] = row.level;
	return {
		...(draft.level === null ? {} : { max_domain_level: draft.level }),
		can_veto_human_consensus: draft.veto,
		reason_min_length: parseReasonLength(draft.reasonLength) ?? 50,
		domain_overrides: overrides
	};
}

export interface DraftErrors extends AdvancedErrors {
	state: string | null;
}

export function validateDraft(draft: Draft): DraftErrors {
	return {
		...validateAdvanced(draft),
		state: draft.onComplete === 'state' && draft.stateKey === null ? ERRORS.domainChoose : null
	};
}

export function draftValid(errors: DraftErrors): boolean {
	return advancedValid(errors) && errors.state === null;
}

/** The request fields of a new connection that its draft sets. */
export function connectFields(
	draft: Draft
): Pick<ConnectInput, 'channel' | 'on_complete_state' | 'on_fail_label_id' | 'governance'> {
	return {
		channel: draft.channel,
		on_complete_state: onCompleteValue(draft),
		on_fail_label_id: draft.failLabel,
		governance: governanceOf(draft)
	};
}

function sameOverrides(a: Record<string, Level>, b: Record<string, Level>): boolean {
	const keys = Object.keys(a);
	return keys.length === Object.keys(b).length && keys.every((key) => a[key] === b[key]);
}

/** Only what differs from the connection, so an unchanged field is never re-sent. */
export function changedFields(
	draft: Draft,
	connection: Connection
): Omit<UpdateInput, 'expected_version'> {
	const out: Omit<UpdateInput, 'expected_version'> = {};
	if (draft.channel !== connection.execution.channel) out.channel = draft.channel;
	const complete = onCompleteValue(draft);
	if (complete !== connection.execution.on_complete_state) out.on_complete_state = complete;
	if (draft.failLabel !== connection.execution.on_fail_label_id)
		out.on_fail_label_id = draft.failLabel;
	const governance = governanceOf(draft);
	const current = connection.governance;
	const changed: GovernanceInput = {};
	if (
		governance.max_domain_level !== undefined &&
		governance.max_domain_level !== current.max_domain_level
	)
		changed.max_domain_level = governance.max_domain_level;
	if (governance.can_veto_human_consensus !== current.can_veto_human_consensus)
		changed.can_veto_human_consensus = governance.can_veto_human_consensus;
	if (governance.reason_min_length !== current.reason_min_length)
		changed.reason_min_length = governance.reason_min_length;
	if (!sameOverrides(governance.domain_overrides ?? {}, current.domain_overrides))
		changed.domain_overrides = governance.domain_overrides ?? {};
	if (Object.keys(changed).length > 0) out.governance = changed;
	return out;
}
