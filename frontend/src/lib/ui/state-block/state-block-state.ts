export type StateKind = 'empty' | 'loading' | 'loadFailed' | 'forbidden';

export interface StateCopy {
	/** Key of the heading; for loading, the screen-reader label. */
	titleKey: string;
	/** Key of the one sentence that says what is kept; none for loading. */
	bodyKey: string | null;
	/** Key of the action label used when the caller gives none; null when the caller must. */
	defaultActionKey: string | null;
	/** Live region role: a failure is announced at once, the rest politely. */
	role: 'status' | 'alert';
}

/** The wording of each state, from the `states.*` catalogue (batch A, T16). */
export function stateCopy(kind: StateKind): StateCopy {
	switch (kind) {
		case 'empty':
			return {
				titleKey: 'states.empty.title',
				bodyKey: 'states.empty.body',
				defaultActionKey: null,
				role: 'status'
			};
		case 'loading':
			return {
				titleKey: 'states.loading.label',
				bodyKey: null,
				defaultActionKey: null,
				role: 'status'
			};
		case 'loadFailed':
			return {
				titleKey: 'states.loadFailed.title',
				bodyKey: 'states.loadFailed.body',
				defaultActionKey: 'states.loadFailed.action',
				role: 'alert'
			};
		case 'forbidden':
			return {
				titleKey: 'states.forbidden.title',
				bodyKey: 'states.forbidden.body',
				defaultActionKey: null,
				role: 'status'
			};
	}
}

/** Capitalises the first letter, for the "{Thing} you add appear here." sentence. */
export function capitalised(text: string): string {
	return text.charAt(0).toUpperCase() + text.slice(1);
}
