/**
 * Record CSV export of the forms page: formula neutralisation, the numeric column exception and
 * the plain-quoting import template.
 *
 * Run standalone: `bun tests/forms-page/filter-expression-export.test.ts`
 */
import { Suite, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, FormRecord } from '../../src/lib/api/forms';
import type { FormsPage } from '../../src/lib/forms/state/forms-page';

await installSvelteSsr();
const { FilterExpressionExportController } =
	await import('../../src/lib/forms/state/filter-expression-export');

const suite = new Suite('forms-page/filter-expression-export');

const fields: FormField[] = [
	{ key: 'note', label: 'Note', type: 'text' },
	{ key: 'qty', label: 'Quantity', type: 'number' }
];

function record(title: string, values: Record<string, unknown>): FormRecord {
	return {
		id: 'r1',
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		title,
		values,
		source: {},
		created_at: '2026-11-20T10:00:00Z',
		updated_at: '2026-11-20T10:00:00Z'
	};
}

function controller() {
	const page = { state: { visibleFields: fields, fields } } as unknown as FormsPage;

	return new FilterExpressionExportController(page);
}

suite.check('recordsCsv neutralises a formula title and a negative number in a text column', () => {
	const csv = controller().recordsCsv([record('=1+1', { note: '-5', qty: -5 })]);

	assertEqual(
		csv,
		'"Record ID","Title","Note","Quantity"\n"r1","\'=1+1","\'-5","-5"',
		'title and text cell prefixed, numeric column keeps its sign'
	);
});

suite.check('recordsCsv neutralises a non-number in a numeric column', () => {
	const csv = controller().recordsCsv([record('t', { qty: '=SUM(A1)' })]);

	assertEqual(csv.split('\n')[1], '"r1","t","","\'=SUM(A1)"', 'formula in numeric column');
});

suite.check('the selected-view and drill-down exports share the neutralised encoder', () => {
	const rows = [record('@x', {})];

	assertEqual(
		controller().viewDraftFilterExpressionCsv(rows),
		controller().recordsCsv(rows),
		'same output'
	);
});

finish(suite.result());
