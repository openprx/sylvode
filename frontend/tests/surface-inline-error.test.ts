/**
 * Inline error helper: maps a failed request to a danger tip, replaces the previous error,
 * clears on success, on navigation and on destroy.
 *
 * Run standalone: `bun tests/surface-inline-error.test.ts`
 */
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr } from './support/svelte-ssr';
import type { SurfaceTip } from '../src/lib/surface/context';

await installSvelteSsr();
const { createInlineError, INLINE_ERROR_TIP_ID } = await import('../src/lib/surface/inline-error');

const suite = new Suite('surface-inline-error');

function setup() {
	const tips: Array<SurfaceTip | null> = [];
	const navigate: Array<() => void> = [];
	const dispose: Array<() => void> = [];
	const errors = createInlineError(
		{ setTip: (tip) => tips.push(tip) },
		{
			translate: (key) => `<${key}>`,
			onNavigate: (cb) => navigate.push(cb),
			onDispose: (cb) => dispose.push(cb)
		}
	);
	return { errors, tips, navigate, dispose };
}

suite.check('a failed result becomes a danger tip with the API message', () => {
	const { errors, tips } = setup();
	assertEqual(errors.check({ code: 500, message: 'Server exploded' }), false, 'reports failure');
	const tip = tips[0];
	assert(tip !== null, 'tip set');
	assertEqual(tip.id, INLINE_ERROR_TIP_ID, 'stable id');
	assertEqual(tip.tone, 'danger', 'tone');
	assertEqual(tip.message, 'Server exploded', 'message');
	assertEqual(tip.action, undefined, 'no retry without a retry callback');
});

suite.check('an empty message falls back to the generic text, or the caller fallback', () => {
	const a = setup();
	a.errors.check({ code: 500, message: '  ' });
	assertEqual(a.tips[0]?.message, '<surface.error.generic>', 'generic');
	const b = setup();
	b.errors.fail('', { fallback: 'Could not load issues.' });
	assertEqual(b.tips[0]?.message, 'Could not load issues.', 'caller fallback');
});

suite.check('retry adds a labelled action that runs the callback', () => {
	const { errors, tips } = setup();
	let runs = 0;
	errors.fail('Offline', { retry: () => void (runs += 1) });
	const action = tips[0]?.action;
	assert(action !== undefined, 'action present');
	assertEqual(action.label, '<surface.error.retry>', 'label');
	void action.run();
	assertEqual(runs, 1, 'retry ran');
});

suite.check('a new error replaces the previous one under the same id', () => {
	const { errors, tips } = setup();
	errors.fail('first');
	errors.fail('second');
	assertEqual(tips.length, 2, 'two sets');
	assertEqual(tips[0]?.id, tips[1]?.id, 'same id, so the host replaces in place');
	assertEqual(tips[1]?.message, 'second', 'latest message');
});

suite.check('success clears a showing error and returns true; with none it does nothing', () => {
	const { errors, tips } = setup();
	assertEqual(errors.check({ code: 0, message: 'ok' }), true, 'success with no error');
	assertEqual(tips.length, 0, 'nothing to clear');
	errors.fail('boom');
	assertEqual(errors.check({ code: 0, message: 'ok' }), true, 'success');
	assertEqual(tips[tips.length - 1], null, 'tip cleared');
});

suite.check('navigation and destroy clear the error; clearing twice sets null once', () => {
	const { errors, tips, navigate, dispose } = setup();
	errors.fail('boom');
	navigate.forEach((cb) => cb());
	assertEqual(tips[tips.length - 1], null, 'cleared on navigation');
	const count = tips.length;
	navigate.forEach((cb) => cb());
	dispose.forEach((cb) => cb());
	assertEqual(tips.length, count, 'no repeat clear');
	errors.fail('again');
	dispose.forEach((cb) => cb());
	assertEqual(tips[tips.length - 1], null, 'cleared on destroy');
});

finish(suite.result());
