/**
 * Redirect targets of the Phase 3 route moves (`src/lib/shell/redirects.ts`): every row of the
 * contract's redirects table, with path parameters, query and fragment kept, and unknown paths
 * left alone.
 *
 * Run standalone: `bun tests/shell-redirects.test.ts`
 */

import { Suite, assertEqual, finish } from './support/harness';
import {
	REDIRECT_STATUS,
	legacyProjectTarget,
	parseLegacyProjectUrl,
	projectPath,
	projectSettingsTarget
} from '../src/lib/shell/redirects';

const suite = new Suite('shell-redirects');
const W = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const P = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';

suite.check('status is a temporary redirect', () => {
	assertEqual(REDIRECT_STATUS, 307, 'case 1');
});

suite.check('/projects/{pid}/audit-reports', () => {
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports`, W),
		`/workspace/${W}/projects/${P}/audit-reports`,
		'case 3'
	);
});

suite.check('/projects/{pid}/audit-reports/{id}', () => {
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports/r-9`, W),
		`/workspace/${W}/projects/${P}/audit-reports/r-9`,
		'case 4'
	);
});

suite.check('query and fragment are preserved', () => {
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports/r-9`, W, '?a=1&b=x%20y', '#section'),
		`/workspace/${W}/projects/${P}/audit-reports/r-9?a=1&b=x%20y#section`,
		'case 5'
	);
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports`, W, 'a=1', 'top'),
		`/workspace/${W}/projects/${P}/audit-reports?a=1#top`,
		'case 6'
	);
});

suite.check('empty query and fragment add nothing', () => {
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports`, W, '?', '#'),
		`/workspace/${W}/projects/${P}/audit-reports`,
		'case 7'
	);
});

suite.check('a trailing slash is accepted', () => {
	assertEqual(
		legacyProjectTarget(`/projects/${P}/audit-reports/`, W),
		`/workspace/${W}/projects/${P}/audit-reports`,
		'case 8'
	);
});

suite.check('unknown paths are untouched', () => {
	for (const path of [
		'/projects',
		`/projects/${P}`,
		`/projects/${P}/issues`,
		`/projects/${P}/archive`,
		`/projects/${P}/audit-reports/a/b`,
		`/workspace/${W}/projects/${P}/audit-reports`,
		'/plugins',
		'/flow/package',
		`/workspace/${W}/settings/flow`
	]) {
		assertEqual(parseLegacyProjectUrl(path), null, path);
		assertEqual(legacyProjectTarget(path, W), null, path);
	}
});

suite.check('a malformed escape is not redirected', () => {
	assertEqual(parseLegacyProjectUrl('/projects/%E0%A4%A/audit-reports'), null, 'case 9');
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
