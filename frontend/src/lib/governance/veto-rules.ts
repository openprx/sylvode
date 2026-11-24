import type { VetoerScope } from '#lib/api/veto.js';

/** The shortest veto reason the server accepts, counted in characters of the trimmed text. */
export const MIN_VETO_REASON = 100;

/** Proposal statuses in which a veto can be exercised. */
const VETOABLE = new Set(['open', 'voting']);

/** The domain key the server stores: trimmed, lower case ASCII, anything else becomes `_`. */
export function normalizeDomainKey(value: string): string {
	return value
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9_-]/g, '_');
}

/** Characters of the trimmed reason (a character is one code point, as the server counts). */
export function reasonLength(reason: string): number {
	return Array.from(reason.trim()).length;
}

export function reasonProblem(reason: string): 'empty' | 'short' | null {
	const length = reasonLength(reason);
	if (length === 0) return 'empty';
	return length < MIN_VETO_REASON ? 'short' : null;
}

/** The domain keys a proposal can be vetoed in: its own domains, or `global` when it has none. */
export function proposalVetoDomains(domains: readonly string[]): string[] {
	const keys = domains.map(normalizeDomainKey).filter((key) => key !== '');
	return keys.length > 0 ? [...new Set(keys)] : ['global'];
}

/**
 * The domains of the proposal in which `userId` is a listed vetoer, in the proposal's order.
 * Empty when the proposal cannot be vetoed any more or the caller is no vetoer there.
 */
export function vetoDomainsFor(
	status: string,
	domains: readonly string[],
	vetoers: readonly Pick<VetoerScope, 'user_id' | 'domain'>[],
	userId: string
): string[] {
	if (!VETOABLE.has(status) || !userId) return [];
	const mine = new Set(vetoers.filter((row) => row.user_id === userId).map((row) => row.domain));
	return proposalVetoDomains(domains).filter((key) => mine.has(key));
}

export function sameVetoer(
	a: Pick<VetoerScope, 'user_id' | 'project_id' | 'domain'>,
	b: Pick<VetoerScope, 'user_id' | 'project_id' | 'domain'>
): boolean {
	return a.user_id === b.user_id && a.project_id === b.project_id && a.domain === b.domain;
}

export interface VetoerFormInput {
	userId: string;
	domain: string;
}

export type VetoerFormProblem = 'member' | 'domain' | 'duplicate';

/** What stops a vetoer from being added, or null when the entry can be sent. */
export function vetoerFormProblem(
	input: VetoerFormInput,
	existing: readonly Pick<VetoerScope, 'user_id' | 'domain'>[]
): VetoerFormProblem | null {
	if (!input.userId) return 'member';
	const domain = normalizeDomainKey(input.domain);
	if (domain === '') return 'domain';
	return existing.some((row) => row.user_id === input.userId && row.domain === domain)
		? 'duplicate'
		: null;
}
