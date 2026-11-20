import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import {
	focusAfterPage,
	layoutFor,
	pageModel,
	positionText,
	rangeText,
	type Translate
} from './pagination-state';

await installSvelteSsr();
const { default: Pagination } = await import('./Pagination.svelte');

const suite = new Suite('ui-pagination');
const dict: Record<string, string> = {
	'ui.pagination.range': 'Showing {from}–{to} of {total}',
	'ui.pagination.rangeNoTotal': 'Showing {from}–{to}',
	'ui.pagination.rangeShort': '{from}–{to} / {total}',
	'ui.pagination.rangeShortNoTotal': '{from}–{to}',
	'ui.pagination.position': 'Page {page} of {pages}',
	'ui.pagination.positionNoTotal': 'Page {page}',
	'ui.pagination.positionShort': '{page} / {pages}',
	'ui.pagination.positionShortNoTotal': '{page}',
	'ui.pagination.loading': 'Loading…'
};
const t: Translate = (key, values) =>
	(dict[key] ?? key).replace(/\{(\w+)\}/g, (_, k: string) => String(values?.[k]));

suite.check('windowing: range and page count for a known total', () => {
	const m = pageModel({ page: 3, pageSize: 50, total: 1240 });
	assertEqual(m.from, 101, 'from');
	assertEqual(m.to, 150, 'to');
	assertEqual(m.pages, 25, 'pages');
	assert(m.render && !m.isFirst && !m.isLast, 'middle page');
});
suite.check('windowing: the last page is partial and last', () => {
	const m = pageModel({ page: 25, pageSize: 50, total: 1240 });
	assertEqual(m.from, 1201, 'from');
	assertEqual(m.to, 1240, 'to is the total');
	assert(m.isLast, 'last');
	assert(pageModel({ page: 1, pageSize: 50, total: 1240 }).isFirst, 'first');
});
suite.check('nothing renders for an empty list or a single page', () => {
	assertEqual(pageModel({ page: 1, pageSize: 20, total: 0 }).render, false, 'empty');
	assertEqual(pageModel({ page: 1, pageSize: 20, total: 20 }).render, false, 'exactly one page');
	assertEqual(pageModel({ page: 1, pageSize: 20, total: 21 }).render, true, 'two pages');
	assertEqual(
		pageModel({ page: 1, pageSize: 20, total: null, hasMore: false }).render,
		false,
		'unknown, no more'
	);
});
suite.check('unknown total: next stays enabled while there is more', () => {
	const m = pageModel({ page: 2, pageSize: 50, total: null, hasMore: true });
	assertEqual(m.pages, null, 'no page count');
	assertEqual(m.isLast, false, 'next enabled');
	assertEqual(m.to, 100, 'to is a full page');
	assertEqual(
		pageModel({ page: 2, pageSize: 50, total: null, hasMore: false, shown: 7 }).to,
		57,
		'shown rows end the range'
	);
	assertEqual(
		pageModel({ page: 2, pageSize: 50, total: null, hasMore: false }).isLast,
		true,
		'last'
	);
});
suite.check('wording: full, short, unknown total, loading', () => {
	const m = pageModel({ page: 3, pageSize: 50, total: 1240 });
	const full = { loading: false, short: false, locale: 'en' };
	assertEqual(rangeText(m, full, t), 'Showing 101–150 of 1,240', 'full range');
	assertEqual(positionText(m, full, t), 'Page 3 of 25', 'position');
	assertEqual(rangeText(m, { ...full, short: true }, t), '101–150 / 1,240', 'short range');
	assertEqual(positionText(m, { ...full, short: true }, t), '3 / 25', 'short position');
	const open = pageModel({ page: 2, pageSize: 50, total: null, hasMore: true });
	assertEqual(rangeText(open, full, t), 'Showing 51–100', 'no total');
	assertEqual(positionText(open, full, t), 'Page 2', 'no page count');
	assertEqual(rangeText(m, { ...full, loading: true }, t), 'Loading…', 'loading');
	assertEqual(
		rangeText(m, { ...full, locale: 'de' }, t),
		'Showing 101–150 of 1.240',
		'numbers follow the app language'
	);
});
suite.check('layout thresholds: size control under 480, short form under 400', () => {
	assertEqual(
		JSON.stringify(layoutFor(600)),
		JSON.stringify({ showSize: true, short: false }),
		'600'
	);
	assertEqual(
		JSON.stringify(layoutFor(480)),
		JSON.stringify({ showSize: true, short: false }),
		'480'
	);
	assertEqual(
		JSON.stringify(layoutFor(479)),
		JSON.stringify({ showSize: false, short: false }),
		'479'
	);
	assertEqual(
		JSON.stringify(layoutFor(340)),
		JSON.stringify({ showSize: false, short: true }),
		'340'
	);
});
suite.check('focus stays on the pressed button unless it just became disabled', () => {
	const mid = pageModel({ page: 3, pageSize: 20, total: 100 });
	const last = pageModel({ page: 5, pageSize: 20, total: 100 });
	const first = pageModel({ page: 1, pageSize: 20, total: 100 });
	assertEqual(focusAfterPage('next', mid), 'next', 'still enabled');
	assertEqual(focusAfterPage('next', last), 'prev', 'reached the last page');
	assertEqual(focusAfterPage('prev', first), 'next', 'reached the first page');
});

await suite.checkAsync('first page: previous is aria-disabled with the extended name', async () => {
	const html = await renderToHtml(Pagination, {
		page: 1,
		pageSize: 20,
		total: 95,
		onpage: () => {}
	});
	assert(html.includes('ui.pagination.previousFirst'), html);
	assert(
		/aria-label="ui.pagination.previousFirst"[^>]*aria-disabled="true"/.test(html),
		'aria-disabled'
	);
	assert(html.includes('aria-label="ui.pagination.next"'), 'next enabled');
	assert(html.includes('<nav') && html.includes('aria-live="polite"'), 'nav and live range');
});
await suite.checkAsync(
	'renders nothing when the total fits one page, keeps extra attributes',
	async () => {
		const none = await renderToHtml(Pagination, {
			page: 1,
			pageSize: 20,
			total: 20,
			onpage: () => {}
		});
		assert(!none.includes('<nav'), none);
		const html = await renderToHtml(Pagination, {
			page: 1,
			pageSize: 20,
			total: 95,
			onpage: () => {},
			'data-record-pagination': ''
		});
		assert(html.includes('data-record-pagination'), html);
	}
);

finish(suite.result());
