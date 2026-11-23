/**
 * Sprint rules the pages share (`src/lib/sprints/rules.ts`): who may start, complete, cancel and
 * delete a sprint, which planned sprints can take rolled-over issues, how past sprints are listed,
 * and which sprints an issue can be moved into. The last checks pin the Cycles page wiring.
 *
 * Run standalone: `bun tests/sprint-rules.test.ts`
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	canManageSprints,
	pastSprints,
	rolloverTargets,
	sprintOptionsForIssue
} from '../src/lib/sprints/rules';
import type { Sprint, SprintStatus } from '../src/lib/api/sprints';

const suite = new Suite('sprint-rules');

function sprint(id: string, status: SprintStatus, end: string | null = null): Sprint {
	return {
		id,
		project_id: 'p',
		name: `Sprint ${id}`,
		start_date: null,
		end_date: end,
		status,
		created_at: '2026-11-01T00:00:00Z',
		updated_at: '2026-11-01T00:00:00Z'
	};
}

const ALL = [
	sprint('a', 'active'),
	sprint('p1', 'planned'),
	sprint('p2', 'planned'),
	sprint('c1', 'completed', '2026-10-31'),
	sprint('c2', 'completed', '2026-11-14'),
	sprint('x1', 'cancelled', '2026-11-20'),
	sprint('x2', 'cancelled')
];

suite.check('only owners and admins manage sprints', () => {
	assertEqual(canManageSprints('owner'), true, 'owner');
	assertEqual(canManageSprints('admin'), true, 'admin');
	assertEqual(canManageSprints('member'), false, 'member');
	assertEqual(canManageSprints(null), false, 'role not known yet');
});

suite.check('rollover targets are the other planned sprints', () => {
	assertDeepEqual(
		rolloverTargets(ALL, 'a').map((s) => s.id),
		['p1', 'p2'],
		'ending the active sprint'
	);
	assertDeepEqual(
		rolloverTargets(ALL, 'p1').map((s) => s.id),
		['p2'],
		'cancelling a planned sprint leaves out itself'
	);
	assertDeepEqual(rolloverTargets([sprint('only', 'active')], 'only'), [], 'no planned sprint');
});

suite.check('past sprints are completed and cancelled ones, latest end first, undated last', () => {
	assertDeepEqual(
		pastSprints(ALL).map((s) => s.id),
		['x1', 'c2', 'c1', 'x2'],
		'order'
	);
});

suite.check('an issue can be moved into planned and active sprints only', () => {
	const options = sprintOptionsForIssue(ALL, null);
	assertDeepEqual(
		options.map((o) => o.id),
		['a', 'p1', 'p2'],
		'ended sprints are not offered'
	);
	assert(
		options.every((o) => o.selectable),
		'every offered sprint can be chosen'
	);
});

suite.check('a completed or cancelled current sprint is shown and cannot be chosen', () => {
	for (const current of ['c1', 'x1']) {
		const options = sprintOptionsForIssue(ALL, current);
		assertDeepEqual(
			options.map((o) => o.id),
			['a', 'p1', 'p2', current],
			`${current}: listed`
		);
		const shown = options.find((o) => o.id === current);
		assertEqual(shown?.selectable, false, `${current}: not selectable`);
		assertEqual(shown?.name, `Sprint ${current}`, `${current}: its name is shown`);
	}
	// An active or planned current sprint is just a normal option.
	const options = sprintOptionsForIssue(ALL, 'a');
	assertEqual(options.filter((o) => o.id === 'a').length, 1, 'no duplicate');
});

suite.check(
	'the Cycles page hides the management buttons from members and sends description',
	() => {
		const page = readFileSync(
			join(
				fileURLToPath(new URL('.', import.meta.url)),
				'../src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/cycles/+page.svelte'
			),
			'utf8'
		);
		const gated = (handler: string): boolean =>
			new RegExp(`\\{#if canManage\\}[\\s\\S]*?${handler}[\\s\\S]*?\\{/if\\}`).test(page);
		for (const handler of [
			"openEndDialog\\(sprint, 'completed'\\)",
			"openEndDialog\\(sprint, 'cancelled'\\)",
			'startSprint\\(sprint\\)',
			'removeSprint\\(sprint\\)'
		]) {
			assert(gated(handler), `${handler} is inside {#if canManage}`);
		}
		assert(/rollover_sprint_id/.test(page), 'the end dialog sends rollover_sprint_id');
		assert(!/\bgoal\b/.test(page), 'no goal field is left');
		assert(/pastSprints\(/.test(page), 'cancelled sprints are listed with the completed ones');
	}
);

suite.check('the issue page has Sprint and Due date controls that send null to clear', () => {
	const page = readFileSync(
		join(
			fileURLToPath(new URL('.', import.meta.url)),
			'../src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte'
		),
		'utf8'
	);
	assert(/sprintOptionsForIssue\(/.test(page), 'the sprint options follow the offering rule');
	assert(
		/updateIssue\(\{ sprint_id: sprintValue \|\| null \}\)/.test(page),
		'no sprint sends null'
	);
	assert(/due_at: dueDateToTimestamp\(next\)/.test(page), 'the due date goes through the mapping');
	assert(/<DatePicker[\s\S]*?onchange=\{handleDueChange\}/.test(page), 'a date picker edits it');
	assert(
		/<Select\s+label=\{\$t\('issue\.sprint'\)\}/.test(page),
		'the sprint control is a labelled select'
	);
});

finish(suite.result());
