/**
 * Workflow state labels (`src/lib/utils/workflow-state.ts`): the four built-in states are shown with
 * translated labels while they carry the server's default name; custom states and renamed ones are
 * shown as stored.
 *
 * Run standalone: `bun tests/workflow-state.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assertEqual, finish } from './support/harness';
import { builtInStateLabelKey, workflowStateLabel } from '../src/lib/utils/workflow-state';

const suite = new Suite('workflow-state');
const translate = (key: string): string => `T(${key})`;

suite.check('built-in states with their default names are translated', () => {
	assertEqual(
		workflowStateLabel({ key: 'backlog', display_name: 'Backlog' }, translate),
		'T(issue.backlog)',
		'backlog'
	);
	assertEqual(
		workflowStateLabel({ key: 'todo', display_name: 'Todo' }, translate),
		'T(issue.todo)',
		'Todo'
	);
	assertEqual(
		workflowStateLabel({ key: 'todo', display_name: 'To Do' }, translate),
		'T(issue.todo)',
		'To Do'
	);
	assertEqual(
		workflowStateLabel({ key: 'in_progress', display_name: 'In Progress' }, translate),
		'T(issue.inProgress)',
		'in progress'
	);
	assertEqual(
		workflowStateLabel({ key: 'done', display_name: 'Done' }, translate),
		'T(issue.done)',
		'done'
	);
	assertEqual(workflowStateLabel({ key: 'done' }, translate), 'T(issue.done)', 'no name');
	assertEqual(
		workflowStateLabel({ key: 'done', display_name: ' ' }, translate),
		'T(issue.done)',
		'blank name'
	);
});

suite.check('custom and renamed states are shown as they are', () => {
	assertEqual(
		workflowStateLabel({ key: 'review', display_name: 'Review' }, translate),
		'Review',
		'custom'
	);
	assertEqual(
		workflowStateLabel({ key: 'ready', display_name: 'Done' }, translate),
		'Done',
		'custom key named Done'
	);
	assertEqual(
		workflowStateLabel({ key: 'done', display_name: 'Shipped' }, translate),
		'Shipped',
		'renamed'
	);
	assertEqual(workflowStateLabel({ key: 'qa' }, translate), 'qa', 'no name falls back to the key');
});

suite.check('only the built-in keys have a label key', () => {
	assertEqual(builtInStateLabelKey('in_progress'), 'issue.inProgress', 'in_progress');
	assertEqual(builtInStateLabelKey('review'), null, 'review');
});

suite.check('the pages that list workflow states use the helper', () => {
	const root = new URL('..', import.meta.url).pathname;
	const project = 'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]';
	for (const file of [
		`${project}/issues/[issueId]/+page.svelte`,
		`${project}/issues/+page.svelte`,
		`${project}/board/+page.svelte`,
		'src/lib/components/IssueCreateModal.svelte'
	]) {
		const source = readFileSync(join(root, file), 'utf8');
		assertEqual(/workflowStateLabel\(/.test(source), true, `${file} uses workflowStateLabel`);
		assertEqual(/display_name \|\| /.test(source), false, `${file} does not show the raw name`);
	}
});

suite.check('an unknown path is answered inside the shell and standalone when signed out', () => {
	const root = new URL('..', import.meta.url).pathname;
	const read = (file: string): string => readFileSync(join(root, file), 'utf8');
	assertEqual(
		/error\(404/.test(read('src/routes/(app)/[...unmatched]/+page.ts')),
		true,
		'catch-all answers 404'
	);
	const layout = read('src/routes/(app)/+layout.ts');
	assertEqual(
		/UNMATCHED_ROUTE\) error\(404/.test(layout),
		true,
		'signed out: 404, not the sign-in redirect'
	);
	assertEqual(/RouteError/.test(read('src/routes/(app)/+error.svelte')), true, 'shell error page');
	assertEqual(/RouteError/.test(read('src/routes/+error.svelte')), true, 'standalone error page');
	assertEqual(
		/StateBlock/.test(read('src/lib/components/RouteError.svelte')),
		true,
		'built on StateBlock'
	);
});

finish(suite.result());
