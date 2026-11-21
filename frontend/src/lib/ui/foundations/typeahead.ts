/** Type-ahead for menus: typed characters jump to the next item whose label starts with them. */

export const TYPEAHEAD_RESET_MS = 500;

/** True for a key that adds to the type-ahead buffer (one printable character, no shortcut). */
export function isTypeaheadKey(event: {
	key: string;
	ctrlKey: boolean;
	metaKey: boolean;
	altKey: boolean;
}): boolean {
	return event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
}

/**
 * Index of the item to focus, or `null` when nothing matches. Matching ignores case and
 * surrounding space. A buffer of one repeated character (`aaa`) cycles through the items that start
 * with it; any other buffer matches by prefix, starting at the current item so typing more letters
 * stays on it while it still fits. Search starts after `current` for a single character.
 */
export function typeaheadMatch(
	labels: readonly string[],
	current: number,
	buffer: string
): number | null {
	const needle = buffer.toLowerCase();
	if (!needle || labels.length === 0) return null;
	const cycling = [...needle].every((ch) => ch === needle[0]);
	const prefix = cycling ? needle[0] : needle;
	const start = cycling ? current + 1 : Math.max(current, 0);
	for (let step = 0; step < labels.length; step += 1) {
		const index = (start + step) % labels.length;
		if (labels[index].trim().toLowerCase().startsWith(prefix)) return index;
	}
	return null;
}

/** Keeps the buffer between key presses and clears it after a pause. */
export class TypeaheadBuffer {
	#text = '';
	#timer: ReturnType<typeof setTimeout> | null = null;
	constructor(private readonly delay = TYPEAHEAD_RESET_MS) {}

	push(char: string): string {
		if (this.#timer) clearTimeout(this.#timer);
		this.#text += char;
		this.#timer = setTimeout(() => this.reset(), this.delay);
		return this.#text;
	}
	reset(): void {
		if (this.#timer) clearTimeout(this.#timer);
		this.#timer = null;
		this.#text = '';
	}
}
