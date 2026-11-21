export type StateKind =
	| 'empty'
	| 'loading'
	| 'noResults'
	| 'noSearchResults'
	| 'offline'
	| 'loadFailed'
	| 'forbidden'
	| 'notFound'
	| 'expired'
	| 'identityChanged';

export interface StateCopy {
	/** Key of the heading; for loading, the screen-reader label. */
	titleKey: string;
	/** Key of the one sentence that says what is kept; none for loading. */
	bodyKey: string | null;
	/** The body key names a plural group (`.one` / `.other`) read with the count. */
	bodyCounted: boolean;
	/** Key of the action label used when the caller gives none; null when the caller must. */
	defaultActionKey: string | null;
	/** Live region role: a failure is announced at once, the rest politely. */
	role: 'status' | 'alert';
}

/** Which of the optional parameters the caller gave; the wording has a generic form without them. */
export interface GivenParams {
	/** The plural object name (`thing`). */
	thing?: boolean;
	/** The singular object name (`object`). */
	object?: boolean;
	/** Both `grantor` and `place`. */
	access?: boolean;
	/** The number of applied filters. */
	count?: boolean;
	/** The `place` to go back to. */
	place?: boolean;
}

const ALL: GivenParams = { thing: true, object: true, access: true, count: true, place: true };

/** The wording of each state, from the `states.*` catalogue (batch A, T16). */
export function stateCopy(kind: StateKind, given: GivenParams = ALL): StateCopy {
	const copy = (
		titleKey: string,
		bodyKey: string | null,
		defaultActionKey: string | null = null,
		role: StateCopy['role'] = 'status',
		bodyCounted = false
	): StateCopy => ({ titleKey, bodyKey, bodyCounted, defaultActionKey, role });
	switch (kind) {
		case 'empty':
			return given.thing
				? copy('states.empty.title', 'states.empty.body')
				: copy('states.empty.titleGeneric', 'states.empty.bodyGeneric');
		case 'loading':
			return copy('states.loading.label', null);
		case 'noResults':
			return copy(
				'states.noResults.title',
				given.count ? 'states.noResults.body' : null,
				'states.noResults.action',
				'status',
				true
			);
		case 'noSearchResults':
			return copy('states.noSearchResults.title', null, 'states.noSearchResults.action');
		case 'offline':
			return copy('states.offline.title', 'states.offline.body', 'states.offline.action');
		case 'loadFailed':
			return copy(
				given.thing ? 'states.loadFailed.title' : 'states.loadFailed.titleGeneric',
				'states.loadFailed.body',
				'states.loadFailed.action',
				'alert'
			);
		case 'forbidden':
			return copy(
				'states.forbidden.title',
				given.access ? 'states.forbidden.body' : 'states.forbidden.bodyGeneric'
			);
		case 'notFound':
			return copy(
				given.object ? 'states.notFound.title' : 'states.notFound.titleGeneric',
				'states.notFound.body',
				given.place ? 'states.notFound.action' : null
			);
		case 'expired':
			return copy('states.expired.title', 'states.expired.body', 'states.expired.action');
		case 'identityChanged':
			return copy(
				'states.identityChanged.title',
				'states.identityChanged.body',
				'states.identityChanged.action'
			);
	}
}

/** Every kind, in catalogue order. */
export const STATE_KINDS: readonly StateKind[] = [
	'empty',
	'loading',
	'noResults',
	'noSearchResults',
	'offline',
	'loadFailed',
	'forbidden',
	'notFound',
	'expired',
	'identityChanged'
];

/** Capitalises the first letter, for the "{Thing} you add appear here." sentence. */
export function capitalised(text: string): string {
	return text.charAt(0).toUpperCase() + text.slice(1);
}
