/**
 * What the sources section shows, as pure helpers: the rules a delete takes with it, the text of
 * an untrusted event field, paging and the values of the one-time credentials screen.
 * No requests, no stores.
 */
import { EVENTS_PAGE } from '#lib/api/inbound.js';
import type { InboundEvent, Rule, Source } from './wire.js';

/** The rules bound to a source: the database cascade deletes them with it. */
export function boundRules(rules: readonly Rule[], sourceId: string): Rule[] {
	return rules.filter((rule) => rule.source_id === sourceId);
}

/** The names the delete dialog lists. */
export function ruleNames(rules: readonly Rule[]): string {
	return rules.map((rule) => rule.name).join(', ');
}

/**
 * Pretty JSON (two-space indentation) of a reported value, for a `<pre>`; empty for no value.
 * It is only ever interpolated as text.
 */
export function reportedJson(value: unknown): string {
	if (value === null || value === undefined) return '';
	try {
		return JSON.stringify(value, null, 2) ?? '';
	} catch {
		return String(value);
	}
}

/** "Show older" is offered while the last page was full. */
export function hasOlder(lastPageSize: number, limit: number = EVENTS_PAGE): boolean {
	return lastPageSize >= limit;
}

/** The cursor of the next page: the `received_at` of the last row. */
export function olderCursor(events: readonly InboundEvent[]): string | null {
	return events.length === 0 ? null : events[events.length - 1].received_at;
}

/** Appends a page, dropping rows already listed (a row may share the cursor's instant). */
export function appendEvents(
	current: readonly InboundEvent[],
	page: readonly InboundEvent[]
): InboundEvent[] {
	const seen = new Set(current.map((event) => event.id));
	return [...current, ...page.filter((event) => !seen.has(event.id))];
}

/** The source of a list with one replaced by its new version. */
export function replaceSource(list: readonly Source[], next: Source): Source[] {
	return list.map((source) => (source.id === next.id ? next : source));
}

export function withoutSource(list: readonly Source[], id: string): Source[] {
	return list.filter((source) => source.id !== id);
}

/** The rule name for an event row: the stored rule's name, `null` for a rule that is gone. */
export function ruleNameOf(rules: readonly Rule[] | null, ruleId: string | null): string | null {
	if (rules === null || ruleId === null) return null;
	return rules.find((rule) => rule.id === ruleId)?.name ?? null;
}
