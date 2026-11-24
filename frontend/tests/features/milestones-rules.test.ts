/**
 * What the milestone screens decide: who may write, progress, name and date handling, the options
 * of the item's milestone select, the failure texts, and reading every milestone of a workspace.
 *
 * Run standalone: `bun test tests/features/milestones-rules.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Milestone } from '../../src/lib/api/milestones';
import {
	MILESTONE_NAME_MAX,
	canManageMilestones,
	formatTargetDate,
	isOverdue,
	loadAllMilestones,
	milestoneActivityText,
	milestoneChoices,
	milestoneFailureMessage,
	milestoneNameOf,
	milestoneProgressOf,
	nameProblem,
	parseDateOnly,
	sortMilestones
} from '../../src/lib/milestones/rules';

const translate = (key: string, options?: { values?: Record<string, unknown> }) =>
	options?.values ? `${key}${JSON.stringify(options.values)}` : key;

const milestone = (over: Partial<Milestone> & { id: string }): Milestone => ({
	workspace_id: 'w1',
	name: over.id,
	description: '',
	target_date: null,
	status: 'open',
	created_at: '2026-11-24T00:00:00Z',
	updated_at: '2026-11-24T00:00:00Z',
	progress: { total: 0, done: 0, by_project: [] },
	...over
});

test('only owners and admins manage milestones', () => {
	assert.equal(canManageMilestones('owner'), true);
	assert.equal(canManageMilestones('admin'), true);
	assert.equal(canManageMilestones('member'), false);
	assert.equal(canManageMilestones(null), false);
});

test('progress comes from the server count and stays in range', () => {
	assert.deepEqual(milestoneProgressOf({ progress: { total: 8, done: 2 } }), {
		total: 8,
		done: 2,
		percent: 25
	});
	assert.deepEqual(milestoneProgressOf({}), { total: 0, done: 0, percent: 0 });
	assert.equal(milestoneProgressOf({ progress: { total: 3, done: 9 } }).done, 3);
});

test('a name is refused when empty or longer than 100 characters', () => {
	assert.equal(nameProblem('   '), 'empty');
	assert.equal(nameProblem('Beta'), null);
	assert.equal(nameProblem('a'.repeat(MILESTONE_NAME_MAX)), null);
	assert.equal(nameProblem('a'.repeat(MILESTONE_NAME_MAX + 1)), 'tooLong');
	assert.equal(nameProblem('里'.repeat(MILESTONE_NAME_MAX)), null, 'characters, not bytes');
});

test('target dates are read as calendar days, without a time-zone shift', () => {
	assert.equal(parseDateOnly('2026-12-01')?.getDate(), 1);
	assert.equal(parseDateOnly('2026-02-30'), null);
	assert.equal(parseDateOnly('2026-12-01T00:00:00Z'), null);
	assert.equal(parseDateOnly(null), null);
	assert.match(formatTargetDate('2026-12-01', 'en'), /12\/01\/2026/);
	assert.match(formatTargetDate('2026-12-01', 'zh'), /2026/);
	assert.equal(formatTargetDate(null, 'en'), '');
});

test('overdue is an open milestone whose day has passed; today is not overdue', () => {
	const now = new Date(2026, 10, 24, 15, 0, 0);
	assert.equal(isOverdue({ status: 'open', target_date: '2026-11-23' }, now), true);
	assert.equal(isOverdue({ status: 'open', target_date: '2026-11-24' }, now), false);
	assert.equal(isOverdue({ status: 'closed', target_date: '2026-11-01' }, now), false);
	assert.equal(isOverdue({ status: 'open', target_date: null }, now), false);
});

test('milestones are listed by target date, undated last, then by name', () => {
	const sorted = sortMilestones([
		milestone({ id: 'c', name: 'C' }),
		milestone({ id: 'b', name: 'B', target_date: '2026-12-31' }),
		milestone({ id: 'a', name: 'A', target_date: '2026-12-01' }),
		milestone({ id: 'd', name: 'A2', target_date: '2026-12-01' })
	]);
	assert.deepEqual(
		sorted.map((m) => m.id),
		['a', 'd', 'b', 'c']
	);
});

test('the select offers open milestones, and a closed current one only as a disabled entry', () => {
	const list = [
		milestone({ id: 'o2', name: 'Release' }),
		milestone({ id: 'o1', name: 'Beta' }),
		milestone({ id: 'x1', name: 'Alpha', status: 'closed' }),
		milestone({ id: 'x2', name: 'Old', status: 'closed' })
	];
	const text = { none: 'No milestone', closed: 'closed' };
	assert.deepEqual(milestoneChoices(list, null, text), [
		{ value: '', label: 'No milestone', disabled: false },
		{ value: 'o1', label: 'Beta', disabled: false },
		{ value: 'o2', label: 'Release', disabled: false }
	]);
	assert.deepEqual(milestoneChoices(list, 'x1', text), [
		{ value: '', label: 'No milestone', disabled: false },
		{ value: 'x1', label: 'Alpha (closed)', disabled: true },
		{ value: 'o1', label: 'Beta', disabled: false },
		{ value: 'o2', label: 'Release', disabled: false }
	]);
});

test('names are found in a loaded list, never made from an id', () => {
	const list = [milestone({ id: 'm1', name: 'Beta' })];
	assert.equal(milestoneNameOf(list, 'm1'), 'Beta');
	assert.equal(milestoneNameOf(list, 'gone'), null);
	assert.equal(milestoneNameOf(list, null), null);
});

test('a taken name has its own text; other failures keep the server message', () => {
	assert.equal(
		milestoneFailureMessage(translate, { message: 'x', error_code: 'milestone_name_taken' }),
		'milestones.error.nameTaken'
	);
	assert.equal(
		milestoneFailureMessage(translate, {
			message: '',
			messageKey: 'transport.networkError',
			messageValues: {}
		}),
		'transport.networkError{}'
	);
	assert.equal(
		milestoneFailureMessage(translate, { message: 'milestone must be an open milestone' }),
		'milestones.error.failed milestone must be an open milestone'
	);
	assert.equal(milestoneFailureMessage(translate, { message: '' }), 'milestones.error.failed');
});

test('the activity line names the milestone when it can, and never prints an id', () => {
	const names = (id: string) => (id === 'm1' ? 'Beta' : null);
	assert.equal(
		milestoneActivityText(translate, { from: null, to: 'm1' }, names),
		'milestones.activity.set{"name":"Beta"}'
	);
	assert.equal(
		milestoneActivityText(translate, { from: 'm1', to: 'm9' }, names),
		'milestones.activity.setUnnamed'
	);
	assert.equal(
		milestoneActivityText(translate, { to: 'm9', to_name: 'Gamma' }, names),
		'milestones.activity.setUnnamed'
	);
	assert.equal(
		milestoneActivityText(translate, { from: 'm1', to: null }, names),
		'milestones.activity.removed'
	);
	assert.equal(milestoneActivityText(translate, undefined, names), 'milestones.activity.removed');
});

test('every milestone of the workspace is read, page by page, both statuses', async () => {
	const seen: string[] = [];
	const lister = async (
		_workspaceId: string,
		params?: { status?: 'open' | 'closed'; page?: number; per_page?: number }
	) => {
		seen.push(`${params?.status}:${params?.page}:${params?.per_page}`);
		const page = params?.page ?? 1;
		return {
			code: 0,
			message: '',
			data: {
				items: [milestone({ id: `${params?.status}${page}`, status: params?.status ?? 'open' })],
				total: 3,
				page,
				per_page: 100,
				total_pages: params?.status === 'open' ? 2 : 1
			}
		};
	};
	const result = await loadAllMilestones(lister, 'w1');
	assert.equal(result.code, 0);
	assert.deepEqual(
		result.data?.map((m) => m.id),
		['open1', 'open2', 'closed1']
	);
	assert.deepEqual(seen, ['open:1:100', 'open:2:100', 'closed:1:100']);
});

test('a failing page ends the read with its failure', async () => {
	const result = await loadAllMilestones(
		async () => ({ code: 500, message: 'boom', data: null }),
		'w1',
		'open'
	);
	assert.equal(result.code, 500);
	assert.equal(result.data, null);
});
