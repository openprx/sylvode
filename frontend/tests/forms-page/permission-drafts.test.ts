/**
 * Role permission defaults and labels (`src/lib/forms/permission-drafts.ts`).
 *
 * Run standalone: `bun tests/forms-page/permission-drafts.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormPermissionPolicy, FormPermissions } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/permission-drafts');

const suite = new Suite('forms-page/permission-drafts');

function policy(
	role: 'owner' | 'admin' | 'member',
	body: FormPermissionPolicy['policy']
): FormPermissionPolicy {
	return {
		id: `pol-${role}`,
		workspace_id: 'ws',
		project_id: 'pr',
		form_id: 'form',
		subject_type: 'role',
		subject_id: role,
		policy: body,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function permissions(policies: FormPermissionPolicy[]): FormPermissions {
	return {
		form_id: 'form',
		policies,
		effective: {
			role: 'member',
			is_bot: false,
			actions: {
				'form.view': true,
				'form.design': true,
				'record.create': true,
				'record.update': true,
				'record.delete': true,
				'record.export': true
			},
			record_scope: 'all',
			fields: {}
		}
	};
}

const allTrue = {
	'form.view': true,
	'form.design': true,
	'record.create': true,
	'record.update': true,
	'record.delete': true,
	'record.export': true
};

suite.check('defaultPermissionActions enables the six actions in order', () => {
	assertDeepEqual(m.defaultPermissionActions(), allTrue, 'all enabled');
	assertDeepEqual(
		Object.keys(m.defaultPermissionActions()),
		[
			'form.view',
			'form.design',
			'record.create',
			'record.update',
			'record.delete',
			'record.export'
		],
		'key order'
	);
	const first = m.defaultPermissionActions();
	first['form.view'] = false;
	assertEqual(m.defaultPermissionActions()['form.view'], true, 'fresh object per call');
});

suite.check('rolePermissionActions overlays the role policy on enabled defaults', () => {
	const p = permissions([
		policy('member', { actions: { 'record.delete': false, 'record.export': false } }),
		policy('admin', { actions: { 'form.design': false } })
	]);
	assertDeepEqual(
		m.rolePermissionActions(p, 'member'),
		{ ...allTrue, 'record.delete': false, 'record.export': false },
		'member'
	);
	assertDeepEqual(
		m.rolePermissionActions(p, 'admin'),
		{ ...allTrue, 'form.design': false },
		'admin'
	);
	assertDeepEqual(m.rolePermissionActions(p, 'owner'), allTrue, 'role without a policy');
	assertDeepEqual(m.rolePermissionActions(permissions([]), 'member'), allTrue, 'no policies');
	assertDeepEqual(
		m.rolePermissionActions(permissions([policy('member', {})]), 'member'),
		allTrue,
		'policy without actions'
	);
	assertDeepEqual(
		m.rolePermissionActions(
			permissions([policy('member', { actions: { 'record.create': true } })]),
			'member'
		),
		allTrue,
		'explicit true'
	);
});

suite.check('roleRecordScope is owned only when the role policy says so', () => {
	const p = permissions([
		policy('member', { record_scope: 'owned' }),
		policy('admin', { record_scope: 'all' }),
		policy('owner', {})
	]);
	assertEqual(m.roleRecordScope(p, 'member'), 'owned', 'member owned');
	assertEqual(m.roleRecordScope(p, 'admin'), 'all', 'admin all');
	assertEqual(m.roleRecordScope(p, 'owner'), 'all', 'owner without scope');
	assertEqual(m.roleRecordScope(permissions([]), 'member'), 'all', 'no policies');
	assertEqual(
		m.roleRecordScope(permissions([policy('admin', { record_scope: 'owned' })]), 'member'),
		'all',
		'another role is ignored'
	);
});

suite.check('permissionActionLabel builds the translation key from the action', () => {
	assertEqual(
		m.permissionActionLabel('form.view'),
		'forms.permissionActions.form_view',
		'form.view'
	);
	assertEqual(
		m.permissionActionLabel('form.design'),
		'forms.permissionActions.form_design',
		'form.design'
	);
	assertEqual(
		m.permissionActionLabel('record.create'),
		'forms.permissionActions.record_create',
		'record.create'
	);
	assertEqual(
		m.permissionActionLabel('record.update'),
		'forms.permissionActions.record_update',
		'record.update'
	);
	assertEqual(
		m.permissionActionLabel('record.delete'),
		'forms.permissionActions.record_delete',
		'record.delete'
	);
	assertEqual(
		m.permissionActionLabel('record.export'),
		'forms.permissionActions.record_export',
		'record.export'
	);
});

suite.check('permissionFieldList joins labels or falls back to the empty text key', () => {
	assertEqual(m.permissionFieldList(['Name', 'Price']), 'Name, Price', 'two labels');
	assertEqual(m.permissionFieldList(['Name']), 'Name', 'one label');
	assertEqual(m.permissionFieldList([]), 'forms.noFieldPolicyEffects', 'none');
});

finish(suite.result());
