/**
 * The rule test dialog without the dialog: the sample event (default text and client-side checks),
 * the request, the state of one test run, and the rows the result is shown as. Pure: no requests,
 * no stores, no markup.
 *
 * Everything an external system may have written (event kinds, the work item title built from
 * them) reaches the screen only as text; this module never builds markup.
 */
import type { CallResult, RuleBody, TestInput } from '#lib/api/inbound.js';
import { testAlertWord, testCauseWord, testReasonWord, testTaskWord, type Word } from './status.js';
import type { RuleVerdict, TestCaps, TestResult } from './wire.js';

// --- sample event ---------------------------------------------------------------------------

/** The only accepted value of the envelope's `spec`. */
export const SAMPLE_SPEC = 'sylvode.inbound_event.v1';
export const SAMPLE_DEFAULT_KIND = 'waf.attack_detected';
/** The sample text is refused above this size, before anything is sent (the payload limit is 32 KiB). */
export const SAMPLE_MAX_BYTES = 64 * 1024;

/** The pre-filled sample: a valid envelope with a fresh event id and the time of now. */
export function sampleText(eventId: string, now: Date, kind?: string | null): string {
	return JSON.stringify(
		{
			spec: SAMPLE_SPEC,
			event_id: eventId,
			occurred_at: now.toISOString(),
			kind: kind && kind.trim() !== '' ? kind : SAMPLE_DEFAULT_KIND,
			severity: 'high',
			title: 'Sample event'
		},
		null,
		2
	);
}

export type SampleError = 'empty' | 'notJson' | 'notObject' | 'tooLarge';

export type SampleParse =
	| { ok: true; envelope: Record<string, unknown> }
	| { ok: false; error: SampleError };

/** Client-side checks of the sample: not empty, within the size cap, valid JSON, an object. */
export function parseSample(text: string): SampleParse {
	if (text.trim() === '') return { ok: false, error: 'empty' };
	if (new TextEncoder().encode(text).length > SAMPLE_MAX_BYTES)
		return { ok: false, error: 'tooLarge' };
	let value: unknown;
	try {
		value = JSON.parse(text);
	} catch {
		return { ok: false, error: 'notJson' };
	}
	if (typeof value !== 'object' || value === null || Array.isArray(value))
		return { ok: false, error: 'notObject' };
	return { ok: true, envelope: value as Record<string, unknown> };
}

// --- request --------------------------------------------------------------------------------

export type TestMode = 'recent' | 'sample';

export interface RequestParts {
	sourceId: string;
	mode: TestMode;
	/** The stored event, in `recent` mode. */
	eventId: string | null;
	/** The parsed sample, in `sample` mode. */
	envelope: Record<string, unknown> | null;
	/** The unsaved rule to include, and the saved rule it stands in for. Top level of the request. */
	draftRule?: RuleBody | null;
	replaces?: string | null;
}

/** The request of one run, or `null` when the chosen mode lacks its input. */
export function buildRequest(parts: RequestParts): TestInput | null {
	const extra: { draft_rule?: RuleBody; replaces?: string } = {};
	if (parts.draftRule) {
		extra.draft_rule = parts.draftRule;
		if (parts.replaces) extra.replaces = parts.replaces;
	}
	if (parts.mode === 'recent')
		return parts.eventId
			? { source_id: parts.sourceId, inbound_event_id: parts.eventId, ...extra }
			: null;
	return parts.envelope ? { source_id: parts.sourceId, envelope: parts.envelope, ...extra } : null;
}

// --- choosing the defaults ------------------------------------------------------------------

/** The preselected source when it is one of the project's, else the first. */
export function defaultSource(
	sources: readonly { id: string }[],
	preselected: string | null | undefined
): string | null {
	if (preselected && sources.some((s) => s.id === preselected)) return preselected;
	return sources[0]?.id ?? null;
}

/** The preselected event when it is among the loaded ones, else the newest (the first). */
export function defaultEvent(
	events: readonly { id: string }[],
	preselected: string | null | undefined
): string | null {
	if (preselected && events.some((e) => e.id === preselected)) return preselected;
	return events[0]?.id ?? null;
}

/** "Recent event" when the source has events, otherwise only a sample can be tested. */
export const defaultMode = (eventCount: number): TestMode => (eventCount > 0 ? 'recent' : 'sample');

// --- state of one run -----------------------------------------------------------------------

/** Where a failure is shown: under the sample field, as "event gone", or in the tip bar. */
export type TestFailure =
	| { place: 'sample'; message: string }
	| { place: 'gone' }
	| { place: 'bar'; message: string; forbidden: boolean };

export type TestState =
	| { phase: 'idle' }
	| { phase: 'running' }
	| { phase: 'result'; result: TestResult }
	| { phase: 'error'; failure: TestFailure };

export const IDLE: TestState = { phase: 'idle' };

/**
 * Starting a run drops whatever the last one showed: while it runs, and when it fails, no earlier
 * result stays on the screen.
 */
export const begin = (): TestState => ({ phase: 'running' });

/** Where a failed call is shown. The backend names the envelope path first for a bad sample. */
export function failureOf(
	result: Extract<CallResult<unknown>, { ok: false }>,
	mode: TestMode
): TestFailure {
	const message = result.message;
	if (result.code === 404 && message.includes('inbound event not found')) return { place: 'gone' };
	if (result.code === 400 && mode === 'sample' && message.startsWith('envelope'))
		return { place: 'sample', message };
	return { place: 'bar', message, forbidden: result.code === 403 };
}

/** The state after the call came back. */
export function settle(result: CallResult<TestResult>, mode: TestMode): TestState {
	return result.ok
		? { phase: 'result', result: result.value }
		: { phase: 'error', failure: failureOf(result, mode) };
}

/** What the sample field shows under itself: a client-side check first, else the server's. */
export function sampleFieldError(state: TestState): string | null {
	return state.phase === 'error' && state.failure.place === 'sample' ? state.failure.message : null;
}

// --- result view ----------------------------------------------------------------------------

export type VerdictView =
	| {
			kind: 'match';
			/** 1-based place in the checked order. */
			position: number;
			name: string;
			draft: boolean;
			alert: Word;
			task: Word;
			cause: Word | null;
	  }
	| { kind: 'noMatch' };

/** The ordinal of a verdict among the rules checked, 1-based, as the person reads the list. */
export function ordinalOf(result: TestResult, verdict: RuleVerdict): number {
	const index = result.rules_evaluated.findIndex((v) => v.id === verdict.id);
	return (index < 0 ? 0 : index) + 1;
}

export function verdictView(result: TestResult): VerdictView {
	const rule = result.rule;
	if (!result.matched || rule === null) return { kind: 'noMatch' };
	return {
		kind: 'match',
		position: ordinalOf(result, rule),
		name: rule.name,
		draft: rule.draft,
		alert: testAlertWord(result.alert),
		task: testTaskWord(result.task),
		cause: result.queue_cause === null ? null : testCauseWord(result.queue_cause)
	};
}

export type RuleRowView = {
	id: string;
	position: number;
	name: string;
	draft: boolean;
} & ({ state: 'matched' } | { state: 'reason'; reason: Word } | { state: 'notChecked' });

/**
 * "Rules checked": the rules in order up to and including the one that handles the event; the
 * engine stops there, so the rules after it are "Not checked". The server evaluates all of them
 * but only what decides is shown as decided. Without a match every rule shows its reason.
 */
export function ruleRows(result: TestResult): RuleRowView[] {
	let decided = false;
	return result.rules_evaluated.map((v, index) => {
		const head = { id: v.id, position: index + 1, name: v.name, draft: v.draft };
		if (decided) return { ...head, state: 'notChecked' as const };
		if (v.matched) {
			decided = true;
			return { ...head, state: 'matched' as const };
		}
		return { ...head, state: 'reason' as const, reason: testReasonWord(v.reason) };
	});
}

export interface LimitRow {
	key: 'tasksHour' | 'active' | 'workspace' | 'alertsHour';
	value: number;
	max: number;
}

export function limitRows(caps: TestCaps): LimitRow[] {
	return [
		{ key: 'tasksHour', value: caps.tasks_last_hour, max: caps.max_tasks_per_hour },
		{ key: 'active', value: caps.active_for_rule, max: caps.max_active_tasks },
		{
			key: 'workspace',
			value: caps.active_in_workspace,
			max: caps.max_active_tasks_per_workspace
		},
		{
			key: 'alertsHour',
			value: caps.alerts_opened_last_hour,
			max: caps.max_alerts_per_rule_per_hour
		}
	];
}
