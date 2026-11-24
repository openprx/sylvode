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
	rolloverMessage,
	rolloverTargets,
	sprintChoices,
	sprintFailureMessage,
	sprintOptionsForIssue,
	type Translate
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

suite.check('past sprints can be listed as completed or cancelled ones separately', () => {
	assertDeepEqual(
		pastSprints(ALL, 'completed').map((s) => s.id),
		['c2', 'c1'],
		'completed'
	);
	assertDeepEqual(
		pastSprints(ALL, 'cancelled').map((s) => s.id),
		['x1', 'x2'],
		'cancelled'
	);
});

// A translator that shows the key and the values, so a test sees which text was chosen.
const tr: Translate = (key, options) =>
	options?.values ? `${key}${JSON.stringify(options.values)}` : key;

suite.check('the issue page and the create dialog offer No Sprint, active and planned', () => {
	const text = { none: 'none', status: (status: SprintStatus) => `st:${status}` };
	const choices = sprintChoices(ALL, null, text);
	assertDeepEqual(
		choices.map((c) => c.value),
		['', 'a', 'p1', 'p2'],
		'ids'
	);
	assertEqual(choices[0].label, 'none', 'the first choice is No Sprint');
	assert(
		choices.every((c) => !c.disabled),
		'all can be chosen'
	);
	const ended = sprintChoices(ALL, 'c1', text).find((c) => c.value === 'c1');
	assertEqual(ended?.label, 'Sprint c1 (st:completed)', 'an ended current sprint is labelled');
	assertEqual(ended?.disabled, true, 'and cannot be chosen');
});

suite.check('rollover message names the target sprint from the server answer', () => {
	const targets = [
		{ id: 'p1', name: 'Sprint 12' },
		{ id: 'p2', name: 'Sprint 13' }
	];
	assertEqual(
		rolloverMessage(tr, 'completed', { moved_count: 3, target_sprint_id: 'p2' }, targets, 'en'),
		'cycles.completed. cycles.rolloverMoved.other{"target":"Sprint 13","count":3}',
		'completed into a planned sprint'
	);
	assertEqual(
		rolloverMessage(tr, 'cancelled', { moved_count: 1, target_sprint_id: 'p1' }, targets, 'en'),
		'cycles.cancelled. cycles.rolloverMoved.one{"target":"Sprint 12","count":1}',
		'cancelled into a planned sprint'
	);
	assertEqual(
		rolloverMessage(tr, 'completed', { moved_count: 2, target_sprint_id: null }, targets, 'en'),
		'cycles.completed. cycles.rolloverMoved.other{"target":"cycles.rolloverBacklog","count":2}',
		'no target is the backlog'
	);
	assertEqual(
		rolloverMessage(tr, 'completed', { moved_count: 0, target_sprint_id: 'p1' }, targets),
		'cycles.completed. cycles.rolloverNone',
		'nothing moved'
	);
	assertEqual(
		rolloverMessage(tr, 'cancelled', undefined, targets),
		'cycles.cancelled. cycles.rolloverNone',
		'no rollover in the answer'
	);
	assertEqual(
		rolloverMessage(tr, 'completed', { moved_count: 2, target_sprint_id: 'gone' }, targets, 'en'),
		'cycles.completed. cycles.rolloverMovedElsewhere.other{"target":null,"count":2}',
		'a target that is not known is never reported as the backlog'
	);
});

suite.check('known sprint refusals get translated texts, unknown ones the generic text', () => {
	const id = '3f9c1a52-0000-4000-8000-000000000001';
	const active = sprintFailureMessage(tr, {
		message: `project already has an active sprint: Sprint One`,
		error_code: 'sprint_active_exists',
		details: { active_sprint_id: id, active_sprint_name: 'Sprint One' }
	});
	assertEqual(active, 'cycles.error.activeExists{"name":"Sprint One"}', 'names the sprint');
	assert(!active.includes(id), 'no id in the text');
	assertEqual(
		sprintFailureMessage(tr, { message: 'x', error_code: 'sprint_active_exists' }),
		'cycles.error.activeExistsUnnamed',
		'a race answers without names'
	);
	assertEqual(
		sprintFailureMessage(tr, {
			message: 'sprint status cannot change from completed to active',
			error_code: 'sprint_invalid_transition',
			details: { from: 'completed', to: 'active' }
		}),
		'cycles.error.invalidTransition{"from":"cycles.status.completed","to":"cycles.status.active"}',
		'invalid transition'
	);
	assertEqual(
		sprintFailureMessage(tr, { message: 'x', error_code: 'sprint_end_before_start' }),
		'cycles.endDateAfterStart',
		'end before start'
	);
	assertEqual(
		sprintFailureMessage(tr, { message: 'x', error_code: 'sprint_name_taken' }),
		'cycles.error.nameTaken',
		'duplicate name'
	);
	assertEqual(
		sprintFailureMessage(tr, { message: 'database error', error_code: 'server_rejected' }),
		'cycles.error.failed{"message":"database error"}',
		'unknown code'
	);
	assertEqual(
		sprintFailureMessage(tr, { message: 'forbidden' }),
		'cycles.error.failed{"message":"forbidden"}',
		'no code'
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
		assert(
			/pastSprints\(sprints, 'completed'\)/.test(page) &&
				/pastSprints\(sprints, 'cancelled'\)/.test(page),
			'completed and cancelled sprints are separate groups'
		);
		assert(/cycles\.cancelledList/.test(page), 'the cancelled group has its own heading');
		assert(!/toast\.error\(response\.message\)/.test(page), 'refusals go through the translator');
		assert(/rolloverMessage\(/.test(page), 'the end toast comes from the helper');
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
	assert(/sprintChoices\(/.test(page), 'the sprint options follow the offering rule');
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

suite.check('the create dialog uses the same sprint offering as the issue page', () => {
	const modal = readFileSync(
		join(
			fileURLToPath(new URL('.', import.meta.url)),
			'../src/lib/components/IssueCreateModal.svelte'
		),
		'utf8'
	);
	assert(/sprintChoices\(/.test(modal), 'the dialog builds its sprint options with the helper');
	assert(!/#each sprints as sprint/.test(modal), 'it no longer lists every sprint');
});

finish(suite.result());
