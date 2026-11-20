import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { headerLayout } from './page-header';

await installSvelteSsr();
const { default: PageHeader } = await import('./PageHeader.svelte');

const suite = new Suite('ui-page-header');

suite.check('wide header keeps up to three secondary actions inline and the full context', () => {
	const wide = headerLayout(1000, 5);
	assertEqual(wide.inlineSecondary, 3, 'three inline');
	assertEqual(wide.contextVisible, true, 'context');
	assertEqual(wide.primaryAsIcon, false, 'primary button');
	assertEqual(headerLayout(1000, 2).inlineSecondary, 2, 'fewer actions');
});
suite.check('at 600 content width every secondary action is in the overflow', () => {
	assertEqual(headerLayout(601, 2).inlineSecondary, 2, '601 still inline');
	assertEqual(headerLayout(600, 2).inlineSecondary, 0, '600');
	assertEqual(headerLayout(520, 2).inlineSecondary, 0, '520');
});
suite.check('below 480 the context is hidden visually and the primary is an icon', () => {
	const narrow = headerLayout(479, 2);
	assertEqual(narrow.contextVisible, false, 'context hidden');
	assertEqual(narrow.primaryAsIcon, true, 'icon');
	const edge = headerLayout(480, 2);
	assertEqual(edge.contextVisible, true, '480 keeps the context');
	assertEqual(edge.primaryAsIcon, false, '480 keeps the button');
});

const run = () => {};
const secondary = [
	{ id: 'a', label: 'Export', kind: 'secondary' as const, run },
	{ id: 'b', label: 'Import', kind: 'secondary' as const, run }
];
const primary = { id: 'new', label: 'New record', kind: 'primary' as const, run };
const context = { segments: [{ id: 'p', label: 'Acme', href: '/p' }], current: 'Leads' };

await suite.checkAsync(
	'title is an h1 that scripts can focus but Tab skips; context describes it',
	async () => {
		const html = await renderToHtml(PageHeader, { title: 'Leads', context, width: 1000 });
		assert(html.includes('<header') && html.includes('<h1'), 'header and h1');
		const h1 = /<h1[^>]*>/.exec(html)?.[0] ?? '';
		assert(h1.includes('tabindex="-1"'), h1);
		const described = /aria-describedby="([^"]+)"/.exec(h1)?.[1];
		assert(described && html.includes(`id="${described}"`), 'description id exists');
	}
);
await suite.checkAsync('standard and compact variants are marked for sizing', async () => {
	const standard = await renderToHtml(PageHeader, { title: 'A', width: 1000 });
	const compact = await renderToHtml(PageHeader, { title: 'A', variant: 'compact', width: 1000 });
	assert(standard.includes('data-variant="standard"'), 'standard');
	assert(compact.includes('data-variant="compact"'), 'compact');
});
await suite.checkAsync(
	'wide: inline secondary buttons and a primary button, no overflow',
	async () => {
		const html = await renderToHtml(PageHeader, { title: 'A', primary, secondary, width: 1000 });
		assert(
			html.includes('Export') && html.includes('Import') && html.includes('New record'),
			'labels'
		);
		assert(!html.includes('aria-haspopup="menu"'), 'no overflow button');
	}
);
await suite.checkAsync('at 600 the secondary actions are only in the overflow menu', async () => {
	const html = await renderToHtml(PageHeader, { title: 'A', primary, secondary, width: 600 });
	assert(!html.includes('>Export<') && !html.includes('>Import<'), 'not drawn as buttons');
	assert(html.includes('aria-haspopup="menu"'), 'overflow button');
});
await suite.checkAsync('at 390 the primary is an icon button named by its label', async () => {
	const html = await renderToHtml(PageHeader, { title: 'A', context, primary, width: 390 });
	assert(html.includes('aria-label="New record"'), 'accessible name equals the label');
	assert(!html.includes('>New record<'), 'no text button');
	assert(/class="context[^"]* ui-sr-only"/.test(html), 'context hidden visually');
	assert(html.includes('<nav'), 'context stays in the DOM');
});
await suite.checkAsync('a disabled primary shows its reason and is described by it', async () => {
	const html = await renderToHtml(PageHeader, {
		title: 'A',
		primary: { ...primary, disabledReason: 'Need access' },
		width: 1000
	});
	const reason = /<span class="reason[^"]*" id="([^"]+)">Need access/.exec(html)?.[1];
	assert(reason, 'visible reason');
	assert(
		html.includes(`aria-describedby="${reason}"`) && html.includes('disabled'),
		'described and disabled'
	);
});
await suite.checkAsync('loading draws skeletons and no actions', async () => {
	const html = await renderToHtml(PageHeader, {
		title: 'A',
		primary,
		secondary,
		loading: true,
		width: 1000
	});
	assert(html.includes('title-skeleton') && html.includes('context-skeleton'), 'skeletons');
	assert(!html.includes('New record') && !html.includes('<h1'), 'no actions, no title');
});

finish(suite.result());
