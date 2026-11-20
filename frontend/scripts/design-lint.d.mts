export type LintRule = 'palette' | 'dark' | 'hex' | 'rounded' | 'confirm' | 'toast';

export type LintCounts = Record<LintRule, number> & { total: number };

export type AllowEntry = { file: string; rules: LintRule[]; reason: string };

export type StaleEntry = { file: string; rule: LintRule; reason: string };

export type LintResult = {
	rows: Array<{ file: string } & LintCounts>;
	totals: LintCounts;
	stale: StaleEntry[];
	rules: LintRule[];
};

export const RULES: Record<LintRule, RegExp>;
export function countSource(text: string, file?: string, keys?: LintRule[]): LintCounts;
export function parseRules(value?: string | boolean): LintRule[];
export function parseAllow(entries: unknown): AllowEntry[];
export function lintTree(
	root: string,
	options?: { rules?: LintRule[]; allow?: AllowEntry[]; cwd?: string }
): Promise<LintResult>;
