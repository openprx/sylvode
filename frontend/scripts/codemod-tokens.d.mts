export type CodemodCounts = {
	pairs: number;
	focus: number;
	shadow: number;
	bold: number;
	unpaired: number;
};

export function transformSource(source: string): { text: string; counts: CodemodCounts };
