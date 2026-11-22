/**
 * CSV cell encoding for client-side exports: formula neutralisation and the numeric exception.
 *
 * Run standalone: `bun tests/forms-csv-export.test.ts`
 */

import {
	csvDownloadBlob,
	csvExportCell,
	isPlainNumber,
	neutralizeCsvFormula
} from '../src/lib/utils/csv';
import { Suite, assert, assertEqual, finish } from './support/harness';

const suite = new Suite('forms-csv-export');

suite.check('every trigger character is neutralised', () => {
	for (const trigger of ['=', '+', '-', '@', '\t', '\r']) {
		assertEqual(neutralizeCsvFormula(`${trigger}1+1`), `'${trigger}1+1`, JSON.stringify(trigger));
	}
	for (const safe of ['', 'plain', 'a=b', '1-2', ' =x']) {
		assertEqual(neutralizeCsvFormula(safe), safe, `unchanged: ${JSON.stringify(safe)}`);
	}
});

suite.check('quoting stays correct after neutralising', () => {
	assertEqual(csvExportCell('=A1'), `"'=A1"`, 'prefix inside quotes');
	assertEqual(csvExportCell('@x"y'), `"'@x""y"`, 'embedded quote doubled');
	assertEqual(csvExportCell('ok'), '"ok"', 'plain value');
});

suite.check('numeric columns keep a plain negative number only', () => {
	for (const ok of ['-5', '-5.25', '-1e-7', '-12.30 CNY', '3']) {
		assertEqual(neutralizeCsvFormula(ok, true), ok, ok);
		assert(isPlainNumber(ok), ok);
	}
	assertEqual(neutralizeCsvFormula('-5', false), "'-5", 'text column neutralises');
	for (const bad of ['-5+cmd', '=1+1', '+5', '-5 cny', '-', '-.5', '- 5']) {
		assertEqual(neutralizeCsvFormula(bad, true), `'${bad}`, `numeric column, ${bad}`);
	}
});

await suite.checkAsync(
	'a downloadable CSV starts with one UTF-8 BOM and keeps its content',
	async () => {
		const csv = `"name"\n"${csvExportCell('=A1')}"\n"报告"`;
		const blob = csvDownloadBlob(csv);
		const bytes = new Uint8Array(await blob.arrayBuffer());
		assertEqual(blob.type, 'text/csv;charset=utf-8', 'media type');
		assertEqual(Array.from(bytes.subarray(0, 3)).join(','), '239,187,191', 'EF BB BF');
		assertEqual(
			new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes),
			`\uFEFF${csv}`,
			'content'
		);
		assert(csv.includes("'=A1"), 'formula protection is still applied before the BOM is added');
		const twice = new Uint8Array(await csvDownloadBlob(`\uFEFF${csv}`).arrayBuffer());
		assertEqual(twice.length, bytes.length, 'a BOM is never doubled');
	}
);

finish(suite.result());
