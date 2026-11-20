export type AvatarKind = 'person' | 'agent' | 'group';
export type AvatarSize = 'sm' | 'md' | 'lg';

/**
 * Initials for a name: the first letter of the first and the last word ("Ada Lovelace" gives
 * "AL"), or the first character of a single word ("Lin" gives "L", "王小明" gives "王").
 */
export function initialsOf(name: string): string {
	const words = name.trim().split(/\s+/).filter(Boolean);
	if (words.length === 0) return '';
	const first = [...words[0]][0] ?? '';
	if (words.length === 1) return first.toUpperCase();
	const last = [...words[words.length - 1]][0] ?? '';
	return `${first}${last}`.toUpperCase();
}

/** Identity by shape, not hue: people are circles, agents rounded squares (specification 3.13). */
export function shapeOf(kind: AvatarKind): 'circle' | 'rounded-square' {
	return kind === 'agent' ? 'rounded-square' : 'circle';
}
