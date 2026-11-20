/**
 * CSV cell encoding for client-side exports: formula neutralisation and the numeric exception.
 *
 * Run standalone: `bun tests/forms-csv-export.test.ts`
 */

import { csvExportCell, isPlainNumber, neutralizeCsvFormula } from '../src/lib/utils/csv';
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

finish(suite.result());
