import { Suite, assert, assertEqual, finish } from './support/harness';
import { toast } from '../src/lib/stores/toast';
import { appToasts } from '../src/lib/ui/toast/app-toasts';
import {
	clearFeedback,
	registerTipSink,
	hasTipSink,
	type FeedbackTip
} from '../src/lib/surface/feedback.svelte';

const suite = new Suite('toast-routing');

function reset() {
	for (const item of [...appToasts.items]) appToasts.dismiss(item.id);
}
function withSink(): { tips: Array<FeedbackTip | null>; stop: () => void } {
	const tips: Array<FeedbackTip | null> = [];
	return { tips, stop: registerTipSink((tip) => tips.push(tip)) };
}

suite.check('success and info become toasts, never tips', () => {
	reset();
	const { tips, stop } = withSink();
	toast.success('Saved');
	toast.info('FYI');
	assertEqual(tips.length, 0, 'no tip');
	const tones = appToasts.items.map((i) => `${i.tone}:${i.message}`).sort();
	assertEqual(tones.join('|'), 'neutral:FYI|success:Saved', 'toasts');
	stop();
});

suite.check('errors go inline as a danger tip and create no toast', () => {
	reset();
	const { tips, stop } = withSink();
	toast.error('Could not load');
	assertEqual(appToasts.items.length, 0, 'no toast');
	assertEqual(tips.length, 1, 'one tip');
	const tip = tips[0];
	assert(tip !== null && tip.tone === 'danger' && tip.message === 'Could not load', 'danger tip');
	assert(tip.id === 'feedback:error' && tip.dismissible === true, 'stable id, dismissible');
	stop();
});

suite.check('a new error replaces the old one in place (same tip id)', () => {
	reset();
	const { tips, stop } = withSink();
	toast.error('first');
	toast.error('second');
	assertEqual(tips.map((t) => t?.id).join(','), 'feedback:error,feedback:error', 'same id');
	assertEqual(tips[1]?.message, 'second', 'latest wins');
	stop();
});

suite.check('a warning takes the slot, but never replaces a standing error', () => {
	reset();
	const { tips, stop } = withSink();
	toast.warning('careful');
	assertEqual(tips[0]?.tone, 'warning', 'warning inline');
	toast.error('broken');
	toast.warning('later warning');
	assertEqual(tips.length, 2, 'warning did not touch the slot');
	assertEqual(appToasts.items.length, 1, 'warning went to a toast');
	assertEqual(appToasts.items[0].tone, 'warning', 'warning tone');
	stop();
});

suite.check('without a tip area errors and warnings fall back to toasts', () => {
	reset();
	assert(!hasTipSink(), 'no sink');
	toast.error('boom');
	toast.warning('hmm');
	const byMessage = new Map(appToasts.items.map((i) => [i.message, i]));
	assertEqual(byMessage.get('boom')?.persistent, true, 'error persists');
	assertEqual(byMessage.get('hmm')?.persistent, false, 'warning times out');
	assertEqual(byMessage.get('boom')?.tone, 'warning', 'toast tone');
});

suite.check('identical messages are deduplicated as toasts', () => {
	reset();
	toast.success('Saved');
	toast.success('Saved');
	toast.error('boom');
	toast.error('boom');
	assertEqual(appToasts.items.length, 2, 'one success, one error');
});

suite.check('clearFeedback clears a shown tip once, and only then', () => {
	reset();
	const { tips, stop } = withSink();
	clearFeedback();
	assertEqual(tips.length, 0, 'nothing shown, nothing cleared');
	toast.error('x');
	clearFeedback();
	clearFeedback();
	assertEqual(tips.length, 2, 'tip then one null');
	assertEqual(tips[1], null, 'null clears');
	stop();
});

suite.check('after the sink is removed feedback falls back again', () => {
	reset();
	const { stop } = withSink();
	stop();
	assert(!hasTipSink(), 'removed');
	toast.error('late');
	assertEqual(appToasts.items.length, 1, 'toast');
});

suite.check('a stale unregister does not remove a newer sink', () => {
	const first = registerTipSink(() => {});
	const second = registerTipSink(() => {});
	first();
	assert(hasTipSink(), 'newer sink survives');
	second();
	assert(!hasTipSink(), 'gone');
});

finish(suite.result());
