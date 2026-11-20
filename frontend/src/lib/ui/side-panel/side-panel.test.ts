import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import {
	PANEL_MIN,
	clampWidth,
	defaultWidth,
	dragWidth,
	maxWidth,
	panelMode,
	separatorWidth
} from './side-panel';

await installSvelteSsr();
const { default: SidePanel } = await import('./SidePanel.svelte');
const { createRawSnippet } = await import('svelte');

const suite = new Suite('ui-side-panel');

suite.check('default width is 600 from a 1280 window, else 480', () => {
	assertEqual(defaultWidth(1280), 600, '1280');
	assertEqual(defaultWidth(1920), 600, '1920');
	assertEqual(defaultWidth(1279), 480, '1279');
	assertEqual(defaultWidth(800), 480, '800');
});
suite.check('maximum width is min(720, 60% of the container), never under the minimum', () => {
	assertEqual(maxWidth(2000), 720, 'capped at 720');
	assertEqual(maxWidth(1000), 600, '60% of 1000');
	assertEqual(maxWidth(400), PANEL_MIN, 'small container');
});
suite.check('clampWidth stays between 360 and the maximum', () => {
	assertEqual(clampWidth(100, 1000), 360, 'min');
	assertEqual(clampWidth(900, 1000), 600, 'max');
	assertEqual(clampWidth(480.4, 1000), 480, 'rounded');
});
suite.check(
	'mode follows the container and viewport widths: 999 overlay, 1000 docked, 767 sheet',
	() => {
		assertEqual(panelMode(999, 1280), 'overlay', '999');
		assertEqual(panelMode(1000, 1280), 'docked', '1000');
		assertEqual(panelMode(1200, 767), 'sheet', 'viewport 767');
		assertEqual(panelMode(500, 768), 'overlay', 'viewport 768 is not a sheet');
	}
);
suite.check(
	'separator keys step by 16 (Shift 64), Home and End clamp, Enter is the default',
	() => {
		assertEqual(separatorWidth('ArrowLeft', false, 480, 1200, 1280), 496, 'left widens');
		assertEqual(separatorWidth('ArrowRight', false, 480, 1200, 1280), 464, 'right narrows');
		assertEqual(separatorWidth('ArrowLeft', true, 480, 1200, 1280), 544, 'shift');
		assertEqual(separatorWidth('ArrowRight', false, 360, 1200, 1280), 360, 'clamped at min');
		assertEqual(separatorWidth('ArrowLeft', true, 700, 1200, 1280), 720, 'clamped at max');
		assertEqual(separatorWidth('Home', false, 500, 1200, 1280), 360, 'home');
		assertEqual(separatorWidth('End', false, 500, 1000, 1280), 600, 'end');
		assertEqual(separatorWidth('Enter', false, 500, 1200, 1280), 600, 'default at 1280');
		assertEqual(separatorWidth('Enter', false, 500, 1200, 1100), 480, 'default below 1280');
		assertEqual(separatorWidth('Tab', false, 500, 1200, 1280), null, 'other keys');
	}
);
suite.check('right-to-left mirrors the arrows', () => {
	assertEqual(separatorWidth('ArrowRight', false, 480, 1200, 1280, true), 496, 'right widens');
});
suite.check('drag width is the distance to the container end, clamped', () => {
	assertEqual(dragWidth(500, 1000, 1000), 500, 'plain');
	assertEqual(dragWidth(900, 1000, 1000), 360, 'min');
	assertEqual(dragWidth(100, 1000, 1000), 600, 'max');
});

const children = createRawSnippet(() => ({ render: () => '<p>body</p>' }));
const base = {
	title: 'Order',
	status: 'ready',
	width: 600,
	onresize: () => {},
	onclose: () => {},
	children
};

await suite.checkAsync(
	'renders an aside with a labelled heading, tools, separator and the mode',
	async () => {
		const html = await renderToHtml(SidePanel, {
			...base,
			identifier: 'LEAD-1',
			windowWidth: 1280,
			containerWidth: 1000
		});
		assert(html.includes('<aside') && html.includes('data-panel-mode="docked"'), 'docked aside');
		assert(/aria-labelledby="ui-panel-title-\d+"/.test(html), 'labelled by the heading');
		assert(html.includes('role="separator"') && html.includes('aria-valuenow="600"'), 'separator');
		assert(html.includes('aria-valuemin="360"') && html.includes('aria-valuemax="600"'), 'limits');
		assert(html.includes('LEAD-1') && html.includes('<p>body</p>'), 'identifier and body');
	}
);
await suite.checkAsync(
	'overlay below 1000 and sheet below 768 (dialog, no separator)',
	async () => {
		const overlay = await renderToHtml(SidePanel, {
			...base,
			windowWidth: 1280,
			containerWidth: 999
		});
		assert(overlay.includes('data-panel-mode="overlay"'), 'overlay');
		const sheet = await renderToHtml(SidePanel, { ...base, windowWidth: 767, containerWidth: 700 });
		assert(sheet.includes('role="dialog"') && sheet.includes('aria-modal="true"'), 'dialog');
		assert(!sheet.includes('role="separator"'), 'no separator in a sheet');
	}
);
await suite.checkAsync('hosted renders no header and no separator', async () => {
	const html = await renderToHtml(SidePanel, { ...base, hosted: true });
	assert(
		!html.includes('<header') && !html.includes('role="separator"') && !html.includes('<h2'),
		'bare'
	);
	assert(html.includes('<p>body</p>'), 'body kept');
});
await suite.checkAsync('a disabled previous tool carries its reason in its name', async () => {
	const html = await renderToHtml(SidePanel, {
		...base,
		prevDisabledReason: 'this is the first record',
		onnext: () => {}
	});
	assert(html.includes('ui.sidePanel.previous, this is the first record'), 'reason in the name');
});
await suite.checkAsync('failed status shows an alert with Retry and hides the body', async () => {
	const html = await renderToHtml(SidePanel, { ...base, status: 'failed', onretry: () => {} });
	assert(html.includes('role="alert"') && html.includes('ui.sidePanel.retry'), 'alert and retry');
	assert(!html.includes('<p>body</p>'), 'no body');
});
await suite.checkAsync(
	'loading sets aria-busy and shows skeletons instead of the body',
	async () => {
		const html = await renderToHtml(SidePanel, { ...base, status: 'loading' });
		assert(html.includes('aria-busy="true"') && !html.includes('<p>body</p>'), 'busy');
	}
);

finish(suite.result());
