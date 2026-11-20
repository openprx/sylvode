/** Match tiers of a local result: exact 0, prefix 1, word prefix 2, substring 3. No fuzzy matching. */
export type MatchTier = 0 | 1 | 2 | 3;

/** Case, width and accent-insensitive form used for matching. */
export function foldForMatch(text: string): string {
	return text.normalize('NFKC').toLowerCase();
}

const WORD_BREAK = /[^\p{L}\p{N}]+/u;

/** The tier at which `text` matches `query`, or null when it does not match at all. */
export function matchTier(query: string, text: string): MatchTier | null {
	const q = foldForMatch(query).trim();
	if (!q) return 3;
	const value = foldForMatch(text);
	if (value === q) return 0;
	if (value.startsWith(q)) return 1;
	if (value.split(WORD_BREAK).some((word) => word.length > 0 && word.startsWith(q))) return 2;
	return value.includes(q) ? 3 : null;
}

export interface Rankable {
	title: string;
	/** Larger is more recent (milliseconds, or a position-derived rank); 0 or absent for never. */
	recency?: number;
}

export type TextCollator = Pick<Intl.Collator, 'compare'>;

/**
 * Orders a local group for a query: match tier, then recency, then locale-aware alphabetical.
 * Items that do not match are dropped. With an empty query nothing is dropped and the input order
 * is kept except that more recent items come first.
 */
export function rankLocal<T extends Rankable>(
	items: readonly T[],
	query: string,
	collator: TextCollator
): T[] {
	const q = query.trim();
	const scored: Array<{ item: T; tier: MatchTier; index: number }> = [];
	items.forEach((item, index) => {
		const tier = matchTier(q, item.title);
		if (tier !== null) scored.push({ item, tier, index });
	});
	scored.sort((a, b) => {
		if (q && a.tier !== b.tier) return a.tier - b.tier;
		const recency = (b.item.recency ?? 0) - (a.item.recency ?? 0);
		if (recency !== 0) return recency;
		if (!q) return a.index - b.index;
		return collator.compare(a.item.title, b.item.title) || a.index - b.index;
	});
	return scored.map((s) => s.item);
}
