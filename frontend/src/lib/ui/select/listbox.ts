/** Pure logic shared by Select and MultiSelect: filtering, the active option, keys, type-ahead. */

export type CategoryColor =
	| 'cat-1'
	| 'cat-2'
	| 'cat-3'
	| 'cat-4'
	| 'cat-5'
	| 'cat-6'
	| 'cat-7'
	| 'cat-8';

export interface SelectOption<V extends string = string> {
	value: V;
	label: string;
	meta?: string;
	color?: CategoryColor;
	disabled?: boolean;
	/** Part of the option's accessible name while it is disabled. */
	disabledReason?: string;
}

/** Above this many options a search field is shown (`searchable: 'auto'`). */
export const SEARCH_THRESHOLD = 8;

export function shouldSearch(searchable: boolean | 'auto', count: number): boolean {
	return searchable === 'auto' ? count > SEARCH_THRESHOLD : searchable;
}

/** Case-insensitive match on the label; an empty query keeps everything. */
export function filterOptions<T extends { label: string }>(
	options: readonly T[],
	query: string
): T[] {
	const needle = query.trim().toLocaleLowerCase();
	if (!needle) return [...options];
	return options.filter((o) => o.label.toLocaleLowerCase().includes(needle));
}

/** The selected option's index, else the first enabled one (else 0), else -1 when empty. */
export function initialActive(
	options: readonly { label?: string; disabled?: boolean }[],
	isSelected: (index: number) => boolean
): number {
	if (options.length === 0) return -1;
	const selected = options.findIndex((_, i) => isSelected(i));
	if (selected >= 0) return selected;
	const enabled = options.findIndex((o) => !o.disabled);
	return enabled >= 0 ? enabled : 0;
}

/** Where a movement key puts the active option; clamped at both ends. `null` for other keys. */
export function moveActive(key: string, active: number, count: number, page = 8): number | null {
	if (count <= 0) return null;
	const clamp = (n: number) => Math.max(0, Math.min(count - 1, n));
	switch (key) {
		case 'ArrowDown':
			return clamp(active + 1);
		case 'ArrowUp':
			return clamp(active < 0 ? 0 : active - 1);
		case 'Home':
			return 0;
		case 'End':
			return count - 1;
		case 'PageDown':
			return clamp(active + page);
		case 'PageUp':
			return clamp(active - page);
		default:
			return null;
	}
}

export interface TypeaheadState {
	buffer: string;
	at: number;
}
export const TYPEAHEAD_RESET_MS = 700;
export const initialTypeahead: TypeaheadState = { buffer: '', at: 0 };

/**
 * Type-ahead over the option labels. Characters typed in quick succession form a prefix; the same
 * character repeated cycles through the options that start with it. Returns the new active
 * index, or `null` when nothing matches (the active option stays).
 */
export function typeahead(
	state: TypeaheadState,
	char: string,
	now: number,
	labels: readonly string[],
	active: number
): { state: TypeaheadState; index: number | null } {
	const buffer =
		(now - state.at > TYPEAHEAD_RESET_MS ? '' : state.buffer) + char.toLocaleLowerCase();
	const next = { buffer, at: now };
	const count = labels.length;
	if (count === 0) return { state: next, index: null };
	const repeated = [...buffer].every((c) => c === buffer[0]);
	const prefix = repeated ? buffer[0] : buffer;
	// A repeated key moves on from the current option; a growing prefix may keep it.
	const first = repeated && buffer.length > 1 ? active + 1 : Math.max(active, 0);
	for (let step = 0; step < count; step += 1) {
		const index = (first + step) % count;
		if (labels[index].toLocaleLowerCase().startsWith(prefix)) return { state: next, index };
	}
	return { state: next, index: null };
}

export type KeyAction =
	| { type: 'open' }
	| { type: 'move'; key: string }
	| { type: 'commit' }
	| { type: 'toggle' }
	| { type: 'toggle-close' }
	| { type: 'close' }
	| { type: 'tab' }
	| { type: 'typeahead'; char: string }
	| null;

export interface KeyInput {
	key: string;
	altKey?: boolean;
	ctrlKey?: boolean;
	metaKey?: boolean;
	open: boolean;
	multiple: boolean;
	/** Focus is in the search field: printable keys and Space are text, not commands. */
	searching: boolean;
	/** The search field is empty: Space then cannot be part of the text, so it may toggle. */
	emptyQuery?: boolean;
}

/** What a key does on the trigger or in the search field (the tables in T2 and T3). */
export function keyAction({
	key,
	altKey,
	ctrlKey,
	metaKey,
	open,
	multiple,
	searching,
	emptyQuery = false
}: KeyInput): KeyAction {
	if (ctrlKey || metaKey) return null;
	if (!open) {
		if (key === 'Enter' || key === ' ' || key === 'ArrowDown') return { type: 'open' };
		if (!altKey && key.length === 1 && !searching) return { type: 'typeahead', char: key };
		return null;
	}
	switch (key) {
		case 'ArrowDown':
		case 'ArrowUp':
			return altKey && key === 'ArrowDown' ? null : { type: 'move', key };
		case 'Home':
		case 'End':
		case 'PageUp':
		case 'PageDown':
			// In a search field Home and End move the caret.
			return searching && (key === 'Home' || key === 'End') ? null : { type: 'move', key };
		case 'Enter':
			return multiple ? { type: 'toggle-close' } : { type: 'commit' };
		case ' ':
			if (searching && !(multiple && emptyQuery)) return null;
			return multiple ? { type: 'toggle' } : { type: 'commit' };
		case 'Escape':
			return { type: 'close' };
		case 'Tab':
			return { type: 'tab' };
		default:
			if (!altKey && key.length === 1 && !searching) return { type: 'typeahead', char: key };
			return null;
	}
}

export const optionId = (listboxId: string, index: number) => `${listboxId}-opt-${index}`;
