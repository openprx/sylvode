import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { formatMetric } from './metric-state';

await installSvelteSsr();
const { default: Metric } = await import('./Metric.svelte');

const suite = new Suite('ui-metric');

suite.check('a missing value is an em dash, text is kept, numbers are grouped', () => {
	assertEqual(formatMetric(null, 'en-US'), '—', 'missing');
	assertEqual(formatMetric('98.6%', 'en-US'), '98.6%', 'text as given');
	assertEqual(formatMetric(1234.5, 'en-US'), '1,234.5', 'grouped');
	assertEqual(
		formatMetric(1234.5, 'en-US', { maximumFractionDigits: 0 }),
		'1,235',
		'format options'
	);
});

await suite.checkAsync('the label comes first and the figure follows it', async () => {
	const html = await renderToHtml(Metric, { label: 'Records', value: 1240, note: 'this week' });
	const label = html.indexOf('Records');
	const figure = html.indexOf('class="figure');
	assert(label >= 0 && figure > label, 'label before figure');
	assert(html.includes('this week'), 'note shown');
});

await suite.checkAsync(
	'loading replaces the figure with a block and marks the group busy',
	async () => {
		const html = await renderToHtml(Metric, { label: 'Records', value: 12, loading: true });
		assert(html.includes('aria-busy="true"'), 'busy');
		assert(html.includes('class="block'), 'loading block');
		assert(!html.includes('>12<'), 'value hidden while loading');
	}
);

finish(suite.result());
