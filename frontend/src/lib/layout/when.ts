import { compile, type CelProgram } from '../forms/rules/index.ts';
import type { RefScope } from './types';

/**
 * Display-only evaluation of a node `when`, a placement `condition` and computed `{cel}` text, in
 * the `layout_when` environment (variables `params`, `data`, `slot`, `viewer`) of the shared rules
 * mirror. The server decides access on every read and action; this only hides, shows or fills in.
 * Any compile or evaluation failure fails closed: hidden for `when`, empty for text.
 */

const CACHE_LIMIT = 256;
const programs = new Map<string, CelProgram | null>();

function program(source: string): CelProgram | null {
	const cached = programs.get(source);
	if (cached !== undefined) return cached;
	const compiled = compile(source, 'layout_when');
	if (programs.size >= CACHE_LIMIT) programs.clear();
	const value = compiled.ok ? compiled.program : null;
	programs.set(source, value);
	return value;
}

function variables(scope: Pick<RefScope, 'params' | 'data' | 'slot' | 'viewer'>) {
	return { params: scope.params, data: scope.data, slot: scope.slot, viewer: scope.viewer };
}

/** Whether a node or placement is shown. No expression means shown; an error means hidden. */
export function evaluateWhen(
	expression: string | null | undefined,
	scope: Pick<RefScope, 'params' | 'data' | 'slot' | 'viewer'>
): boolean {
	if (typeof expression !== 'string' || expression.trim() === '') return true;
	const compiled = program(expression);
	if (!compiled) return false;
	const result = compiled.evalBool(variables(scope));
	return result.ok && result.value;
}

/** The text of a computed `{cel}` value; empty on any error or a non-scalar result. */
export function evaluateText(
	expression: string,
	scope: Pick<RefScope, 'params' | 'data' | 'slot' | 'viewer'>
): string {
	const compiled = program(expression);
	if (!compiled) return '';
	const result = compiled.eval(variables(scope));
	if (!result.ok) return '';
	const value = result.value;
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'bigint' || typeof value === 'boolean') {
		return String(value);
	}
	return '';
}
