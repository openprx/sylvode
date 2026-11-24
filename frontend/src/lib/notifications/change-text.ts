// Inbox text of the two notification kinds a work-item change writes. The server stores the content as
// `<kind>:<title>|<old>|<new>`; a title may contain `|`, so the two values are cut from the right.

export type ChangeKind = 'issue_state_changed' | 'issue_priority_changed';

export interface ParsedChange {
	kind: ChangeKind;
	title: string;
	old: string;
	next: string;
}

export const CHANGE_KINDS: readonly ChangeKind[] = [
	'issue_state_changed',
	'issue_priority_changed'
];

export function isChangeKind(kind: string | null | undefined): kind is ChangeKind {
	return CHANGE_KINDS.includes(kind as ChangeKind);
}

/** The parts of a change content, or null when the text is not of the documented shape. */
export function parseChangeContent(kind: string, content: string): ParsedChange | null {
	if (!isChangeKind(kind)) return null;
	const prefix = `${kind}:`;
	if (!content.startsWith(prefix)) return null;
	const body = content.slice(prefix.length);
	const last = body.lastIndexOf('|');
	if (last < 0) return null;
	const middle = body.lastIndexOf('|', last - 1);
	if (middle < 0) return null;
	return {
		kind,
		title: body.slice(0, middle),
		old: body.slice(middle + 1, last),
		next: body.slice(last + 1)
	};
}

export const TITLE_KEYS: Record<ChangeKind, string> = {
	issue_state_changed: 'notification.issueStateChangedTitle',
	issue_priority_changed: 'notification.issuePriorityChangedTitle'
};

export const CONTENT_KEYS: Record<ChangeKind, string> = {
	issue_state_changed: 'notification.issueStateChangedContent',
	issue_priority_changed: 'notification.issuePriorityChangedContent'
};

export const FALLBACK_KEYS: Record<ChangeKind, string> = {
	issue_state_changed: 'notification.issueStateChangedFallback',
	issue_priority_changed: 'notification.issuePriorityChangedFallback'
};

const STATE_KEYS: Record<string, string> = {
	backlog: 'issue.backlog',
	todo: 'issue.todo',
	in_progress: 'issue.inProgress',
	done: 'issue.done'
};

const PRIORITY_KEYS: Record<string, string> = {
	low: 'issue.low',
	medium: 'issue.medium',
	high: 'issue.high',
	urgent: 'issue.urgent'
};

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (
	key: string,
	options?: { values?: Record<string, string | number> }
) => string;

/** A state or priority value in words; a value without words (a custom workflow state) is shown as it is. */
export function valueLabel(kind: ChangeKind, value: string, translate: Translate): string {
	const key = (kind === 'issue_state_changed' ? STATE_KEYS : PRIORITY_KEYS)[value];
	return key ? translate(key) : value;
}

/** The inbox line of a change notification: item, old value, new value; a fixed text when malformed. */
export function changeContentText(kind: ChangeKind, content: string, translate: Translate): string {
	const parsed = parseChangeContent(kind, content);
	if (!parsed) return translate(FALLBACK_KEYS[kind]);
	return translate(CONTENT_KEYS[kind], {
		values: {
			title: parsed.title,
			old: valueLabel(kind, parsed.old, translate),
			new: valueLabel(kind, parsed.next, translate)
		}
	});
}
