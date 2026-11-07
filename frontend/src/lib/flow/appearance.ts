/** Gallery covers rendered in the page header and on a public link. */
export const COVER_GALLERIES = [
	{ id: 'dawn', background: 'linear-gradient(120deg, #f6d365 0%, #fda085 55%, #f6d365 100%)' },
	{ id: 'ocean', background: 'linear-gradient(120deg, #89f7fe 0%, #66a6ff 100%)' },
	{ id: 'moss', background: 'linear-gradient(120deg, #d4fc79 0%, #96e6a1 100%)' },
	{ id: 'dusk', background: 'linear-gradient(120deg, #a18cd1 0%, #fbc2eb 100%)' },
	{ id: 'sand', background: 'linear-gradient(120deg, #fdfbfb 0%, #ebedee 40%, #d7ccc8 100%)' },
	{ id: 'ink', background: 'linear-gradient(120deg, #434343 0%, #000000 100%)' }
] as const;

export function galleryBackground(id: string): string | null {
	return COVER_GALLERIES.find((entry) => entry.id === id)?.background ?? null;
}

export const PAGE_EMOJIS = [
	'📄',
	'📝',
	'📚',
	'✨',
	'💡',
	'🎯',
	'🚀',
	'🌿',
	'🌊',
	'🔥',
	'⭐',
	'🎨',
	'🧩',
	'📌',
	'🗂️',
	'✅'
];
