import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { collapseSegments, contextString } from './context-line';

await installSvelteSsr();
const { default: ContextLine } = await import('./ContextLine.svelte');

const suite = new Suite('ui-context-line');

suite.check('contextString joins labels and the current item with " / "', () => {
	assertEqual(
		contextString(
			[
				{ id: 'a', label: 'A' },
				{ id: 'b', label: 'B' }
			],
			'C'
		),
		'A / B / C',
		'joined'
	);
	assertEqual(contextString([], 'Projects'), 'Projects', 'current alone');
});

const widths = (n: number, each = 100) => Array.from({ length: n }, () => each);

suite.check('nothing collapses when everything fits', () => {
	assertDeepEqual(
		collapseSegments({ segments: widths(4), current: 100, ellipsis: 30, available: 500 }),
		[],
		'hidden'
	);
});
suite.check('collapse starts at the second segment and stops as soon as the line fits', () => {
	// 4 ancestors + current = 500; with one hidden: 400 + 30 = 430.
	const hidden = collapseSegments({
		segments: widths(4),
		current: 100,
		ellipsis: 30,
		available: 440
	});
	assertDeepEqual(hidden, [1], 'one hidden');
	const two = collapseSegments({ segments: widths(4), current: 100, ellipsis: 30, available: 340 });
	assertDeepEqual(two, [1, 2], 'two hidden, parent (index 3) kept');
});
suite.check('first segment, parent and current are kept while anything else can go', () => {
	const hidden = collapseSegments({
		segments: widths(5),
		current: 100,
		ellipsis: 30,
		available: 1
	});
	assert(
		!hidden.includes(0) && hidden.includes(1) && hidden.includes(2) && hidden.includes(3),
		'middle hidden'
	);
});
suite.check(
	'when first, ellipsis, parent and current still do not fit, the parent goes too',
	() => {
		const hidden = collapseSegments({
			segments: widths(5),
			current: 100,
			ellipsis: 30,
			available: 150
		});
		assertDeepEqual(hidden, [1, 2, 3, 4], 'parent hidden as well');
		assert(!hidden.includes(0), 'first stays');
	}
);
suite.check('a single ancestor is never hidden by the middle rule', () => {
	assertDeepEqual(
		collapseSegments({ segments: widths(1), current: 100, ellipsis: 30, available: 50 }),
		[],
		'one'
	);
});

await suite.checkAsync(
	'renders nav, an ordered list with n+1 items and aria-current on the last',
	async () => {
		const html = await renderToHtml(ContextLine, {
			segments: [
				{ id: 'a', label: 'Acme', href: '/a' },
				{ id: 'b', label: 'Forms', href: '/b' }
			],
			current: 'Leads'
		});
		const nav = /<nav[\s\S]*?<\/nav>/.exec(html)?.[0] ?? '';
		assert(nav.includes('aria-label="ui.contextLine.label"'), nav);
		assertEqual((nav.match(/<li/g) ?? []).length, 3, 'items');
		const last = nav.slice(nav.lastIndexOf('<li'));
		assert(last.includes('aria-current="page"') && !last.includes('<a'), last);
	}
);
await suite.checkAsync(
	'a restricted ancestor renders no link and names the missing access',
	async () => {
		const html = await renderToHtml(ContextLine, {
			segments: [{ id: 'r', label: 'Secret', href: '/r', state: 'restricted' }],
			current: 'Leads'
		});
		const nav = /<nav[\s\S]*?<\/nav>/.exec(html)?.[0] ?? '';
		assert(!nav.includes('<a'), 'no link');
		assert(nav.includes('ui.contextLine.noAccess'), 'access sentence');
		assert(!nav.includes('Secret'), 'title is not revealed');
	}
);

finish(suite.result());
