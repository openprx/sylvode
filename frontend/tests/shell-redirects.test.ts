/**
 * Project path builders and the settings landing target (`src/lib/shell/redirects.ts`), with path
 * parameters encoded and query and fragment kept.
 *
 * Run standalone: `bun tests/shell-redirects.test.ts`
 */

import { Suite, assertEqual, finish } from './support/harness';
import { REDIRECT_STATUS, projectPath, projectSettingsTarget } from '../src/lib/shell/redirects';

const suite = new Suite('shell-redirects');
const W = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const P = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';

suite.check('status is a temporary redirect', () => {
	assertEqual(REDIRECT_STATUS, 307, 'case 1');
});

suite.check('path parameters are encoded in the target', () => {
	assertEqual(
		projectPath('a b', 'c/d', 'forms'),
		'/workspace/a%20b/projects/c%2Fd/forms',
		'case 10'
	);
});

suite.check('project settings landing goes to plugins for every type', () => {
	assertEqual(
		projectSettingsTarget(W, P, 'custom_form'),
		`/workspace/${W}/projects/${P}/settings/plugins`,
		'case 11'
	);
	assertEqual(
		projectSettingsTarget(W, P, 'software'),
		`/workspace/${W}/projects/${P}/settings/plugins`,
		'case 12'
	);
	assertEqual(
		projectSettingsTarget(W, P, undefined),
		`/workspace/${W}/projects/${P}/settings/plugins`,
		'case 13'
	);
});

suite.check('project settings landing keeps query and fragment', () => {
	assertEqual(
		projectSettingsTarget(W, P, 'custom_form', '?x=1', '#h'),
		`/workspace/${W}/projects/${P}/settings/plugins?x=1#h`,
		'case 14'
	);
});

finish(suite.result());
