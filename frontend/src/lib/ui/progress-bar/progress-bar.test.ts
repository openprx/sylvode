import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { announcementFor, clampValue, percentOf, stepOf } from './progress-state';

await installSvelteSsr();
const { default: ProgressBar } = await import('./ProgressBar.svelte');

const suite = new Suite('ui-progress-bar');

suite.check('the value is clamped to 0..max and NaN counts as 0', () => {
	assertEqual(clampValue(-5, 100), 0, 'below zero');
	assertEqual(clampValue(140, 100), 100, 'above max');
	assertEqual(clampValue(Number.NaN, 100), 0, 'NaN');
	assertEqual(clampValue(3, 8), 3, 'inside');
	assertEqual(clampValue(3, 0), 0, 'no range');
});

suite.check('the percentage is rounded from the clamped value', () => {
	assertEqual(percentOf(3, 8), 38, '3 of 8 is 38%');
	assertEqual(percentOf(250, 100), 100, 'clamped to 100%');
	assertEqual(percentOf(1, 0), 0, 'max zero');
});

suite.check('milestones: start, each quarter, completion and failure, nothing else', () => {
	assertEqual(announcementFor(null, { state: 'running', step: 0 }), null, 'first render is silent');
	assertEqual(
		announcementFor({ state: 'queued', step: 0 }, { state: 'running', step: 0 }),
		'started',
		'start'
	);
	assertEqual(
		announcementFor({ state: 'running', step: 0 }, { state: 'running', step: 1 }),
		'milestone',
		'quarter'
	);
	assertEqual(
		announcementFor({ state: 'running', step: 1 }, { state: 'running', step: 1 }),
		null,
		'within a quarter is silent'
	);
	assertEqual(
		announcementFor({ state: 'running', step: 3 }, { state: 'done', step: 4 }),
		'done',
		'completion'
	);
	assertEqual(
		announcementFor({ state: 'running', step: 1 }, { state: 'failed', step: 1 }),
		'failed',
		'failure'
	);
	assertEqual(stepOf(100), 4, 'complete is step 4');
});

await suite.checkAsync('the bar is a progressbar with its range and spoken value', async () => {
	const html = await renderToHtml(ProgressBar, {
		value: 3,
		max: 8,
		label: 'Import',
		valueText: '3 of 8 files'
	});
	assert(html.includes('role="progressbar"'), 'role');
	assert(html.includes('aria-valuemin="0"'), 'min');
	assert(html.includes('aria-valuemax="8"'), 'max');
	assert(html.includes('aria-valuenow="3"'), 'now');
	assert(html.includes('aria-valuetext="3 of 8 files"'), 'value text');
	assert(html.includes('aria-label="Import"'), 'name');
});

await suite.checkAsync('a failed bar shows the message in danger with a Retry action', async () => {
	const html = await renderToHtml(ProgressBar, {
		value: 40,
		state: 'failed',
		message: 'Stopped at 40%.',
		onretry: () => {}
	});
	assert(/class="message[^"]*failed/.test(html), 'failed message');
	assert(html.includes('Stopped at 40%.'), 'message text');
	assert(html.includes('ui.progress.retry'), 'retry action');
	assert(html.includes('aria-valuenow="40"'), 'value kept');
});

await suite.checkAsync('a queued bar without a message gets the waiting sentence', async () => {
	const html = await renderToHtml(ProgressBar, { value: 0, state: 'queued' });
	assert(html.includes('ui.progress.queued'), 'default queued message');
	assert(!html.includes('ui.progress.retry'), 'no retry when not failed');
});

finish(suite.result());
