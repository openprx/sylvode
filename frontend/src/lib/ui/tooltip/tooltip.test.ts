import { Suite, assertEqual, finish } from '../../../../tests/support/harness';
import { createTooltipController, type TimerApi } from './tooltip-controller';

class FakeTimers implements TimerApi {
	now = 0;
	private next = 1;
	private pending = new Map<number, { at: number; fn: () => void }>();
	setTimeout(fn: () => void, ms: number) {
		const id = this.next++;
		this.pending.set(id, { at: this.now + ms, fn });
		return id;
	}
	clearTimeout(handle: unknown) {
		this.pending.delete(handle as number);
	}
	advance(ms: number) {
		this.now += ms;
		for (const [id, timer] of [...this.pending]) {
			if (timer.at <= this.now) {
				this.pending.delete(id);
				timer.fn();
			}
		}
	}
}

function setup() {
	const timers = new FakeTimers();
	const log: boolean[] = [];
	const controller = createTooltipController({ onchange: (v) => log.push(v), timers });
	return { timers, log, controller };
}

const suite = new Suite('ui-tooltip');

suite.check('hover shows after 500 ms, not before', () => {
	const { timers, controller } = setup();
	controller.pointerEnter();
	timers.advance(499);
	assertEqual(controller.visible, false, 'at 499');
	timers.advance(1);
	assertEqual(controller.visible, true, 'at 500');
});
suite.check('leaving before the delay cancels it', () => {
	const { timers, controller } = setup();
	controller.pointerEnter();
	timers.advance(300);
	controller.pointerLeave();
	timers.advance(500);
	assertEqual(controller.visible, false, 'never shown');
});
suite.check('keyboard focus shows at once; blur hides at once', () => {
	const { controller } = setup();
	controller.focus();
	assertEqual(controller.visible, true, 'shown');
	controller.hide();
	assertEqual(controller.visible, false, 'hidden');
});
suite.check('leave hides after the 100 ms grace', () => {
	const { timers, controller } = setup();
	controller.focus();
	controller.pointerLeave();
	timers.advance(99);
	assertEqual(controller.visible, true, 'inside grace');
	timers.advance(1);
	assertEqual(controller.visible, false, 'after grace');
});
suite.check('the pointer may move onto the tooltip, which keeps it open', () => {
	const { timers, controller } = setup();
	controller.focus();
	controller.pointerLeave();
	controller.tooltipEnter();
	timers.advance(1000);
	assertEqual(controller.visible, true, 'kept while hovered');
	controller.tooltipLeave();
	timers.advance(100);
	assertEqual(controller.visible, false, 'hidden after leaving it');
});
suite.check('Escape (hide) works while visible and reports each change once', () => {
	const { log, controller } = setup();
	controller.focus();
	controller.focus();
	controller.hide();
	controller.hide();
	assertEqual(log.join(','), 'true,false', 'changes');
});

finish(suite.result());
