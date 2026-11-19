/**
 * Which view a form opens with on a cold load, and the per-user memory behind it.
 *
 * Run standalone: `bun tests/forms-view-precedence.test.ts`
 */

import {
	ALL_RECORDS_VIEW_ID,
	forgetRememberedView,
	readRememberedView,
	resolveInitialView,
	writeRememberedView
} from '../src/lib/forms/view-precedence';
import { Suite, assertDeepEqual, assertEqual, finish } from './support/harness';
import { MemoryStore } from './support/memory-store';

const suite = new Suite('forms-view-precedence');

const grid = { id: 'v-grid', config: {} };
const detail = { id: 'v-detail', config: {} };
const adminDefault = { id: 'v-default', config: { is_default: true } };

suite.check('no views gives the built-in table', () => {
	assertDeepEqual(
		resolveInitialView({ views: [] }),
		{ viewId: ALL_RECORDS_VIEW_ID, source: 'built-in', dropRemembered: false },
		'empty form'
	);
});

suite.check('template views without a default still open the table', () => {
	assertEqual(resolveInitialView({ views: [detail] }).viewId, ALL_RECORDS_VIEW_ID, 'only Detail');
	assertEqual(
		resolveInitialView({ views: [detail, grid] }).viewId,
		ALL_RECORDS_VIEW_ID,
		'Detail and Grid'
	);
});

suite.check('an admin default outranks the table', () => {
	assertDeepEqual(
		resolveInitialView({ views: [grid, adminDefault] }),
		{ viewId: 'v-default', source: 'form-default', dropRemembered: false },
		'default view'
	);
});

suite.check('a URL view wins over memory and default', () => {
	const resolved = resolveInitialView({
		views: [grid, adminDefault],
		urlViewId: 'v-grid',
		rememberedViewId: 'v-default'
	});
	assertDeepEqual(resolved, { viewId: 'v-grid', source: 'url', dropRemembered: false }, 'url');
});

suite.check('?view=all opens the table and keeps the remembered choice', () => {
	const resolved = resolveInitialView({
		views: [detail],
		urlViewId: 'all',
		rememberedViewId: 'v-detail'
	});
	assertDeepEqual(
		resolved,
		{ viewId: ALL_RECORDS_VIEW_ID, source: 'url', dropRemembered: false },
		'all'
	);
});

suite.check('an unknown URL view is ignored', () => {
	const resolved = resolveInitialView({
		views: [detail],
		urlViewId: 'deleted',
		rememberedViewId: 'v-detail'
	});
	assertDeepEqual(
		resolved,
		{ viewId: 'v-detail', source: 'remembered', dropRemembered: false },
		'falls through'
	);
});

suite.check('remembered beats the admin default', () => {
	assertEqual(
		resolveInitialView({ views: [grid, adminDefault], rememberedViewId: 'v-grid' }).source,
		'remembered',
		'remembered'
	);
});

suite.check('a remembered table beats the admin default', () => {
	assertDeepEqual(
		resolveInitialView({ views: [adminDefault], rememberedViewId: 'all' }),
		{ viewId: ALL_RECORDS_VIEW_ID, source: 'remembered', dropRemembered: false },
		'all remembered'
	);
});

suite.check('a deleted remembered view is dropped and the default applies', () => {
	assertDeepEqual(
		resolveInitialView({ views: [adminDefault], rememberedViewId: 'gone' }),
		{ viewId: 'v-default', source: 'form-default', dropRemembered: true },
		'deleted'
	);
	assertDeepEqual(
		resolveInitialView({ views: [grid], rememberedViewId: 'gone' }),
		{ viewId: ALL_RECORDS_VIEW_ID, source: 'built-in', dropRemembered: true },
		'deleted, no default'
	);
});

suite.check('a view the user can no longer see is skipped like a deleted one', () => {
	// The API never lists another user's private view, so it is simply absent from `views`.
	const visible = [grid];
	const resolved = resolveInitialView({
		views: visible,
		rememberedViewId: 'someone-elses-private'
	});
	assertEqual(resolved.viewId, ALL_RECORDS_VIEW_ID, 'skipped');
	assertEqual(resolved.dropRemembered, true, 'dropped');
});

suite.check('blank ids count as no input', () => {
	assertDeepEqual(
		resolveInitialView({ views: [grid], urlViewId: '  ', rememberedViewId: '' }),
		{ viewId: ALL_RECORDS_VIEW_ID, source: 'built-in', dropRemembered: false },
		'blank'
	);
});

suite.check('memory is per user and per form', () => {
	const store = new MemoryStore();
	writeRememberedView('alice', 'form-1', 'v-detail', store);
	assertEqual(readRememberedView('alice', 'form-1', store), 'v-detail', 'alice, form-1');
	assertEqual(
		readRememberedView('bob', 'form-1', store),
		null,
		"another user's choice is not read"
	);
	assertEqual(readRememberedView('alice', 'form-2', store), null, 'another form');
	forgetRememberedView('alice', 'form-1', store);
	assertEqual(readRememberedView('alice', 'form-1', store), null, 'forgotten');
});

suite.check('corrupted or blocked storage reads as no memory', () => {
	const store = new MemoryStore();
	store.setItem('sv-forms-last-view:v1:alice:form-1', '{not json');
	assertEqual(readRememberedView('alice', 'form-1', store), null, 'invalid JSON');
	store.setItem('sv-forms-last-view:v1:alice:form-1', JSON.stringify({ viewId: 42 }));
	assertEqual(readRememberedView('alice', 'form-1', store), null, 'wrong shape');
	assertEqual(readRememberedView('alice', 'form-1', null), null, 'no storage');
	store.failWrites = true;
	writeRememberedView('alice', 'form-1', 'v', store);
});

finish(suite.result());
