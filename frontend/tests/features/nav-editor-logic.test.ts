/**
 * Editing rules of a navigation document (`lib/nav/editor/logic.ts`).
 *
 * Run standalone: `bun test tests/features/nav-editor-logic.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import {
	addEntry,
	addSection,
	cleanLabels,
	externalUrlOk,
	freeBuiltins,
	moveEntry,
	removeSection,
	stepEntry,
	stepSection,
	uniqueId
} from '../../src/lib/nav/editor/logic';
import type { NavDoc, NavEntryDoc } from '../../src/lib/nav/types';

const entry = (id: string, page = `project.${id}`): NavEntryDoc => ({
	id,
	target: { kind: 'builtin', page },
	label: null,
	icon: null,
	hidden: false,
	pinned: false,
	locked: false
});

const doc: NavDoc = {
	schema: 'sv.nav.v1',
	scope: 'project',
	sections: [
		{
			id: 'main',
			label: null,
			collapsed: false,
			entries: [entry('issues'), entry('board'), entry('cycles')]
		},
		{ id: 'more', label: { en: 'More' }, collapsed: true, entries: [entry('forms')] }
	]
};
const ids = (d: NavDoc) => d.sections.map((s) => s.entries.map((e) => e.id));

test('an entry moves within a section and across sections', () => {
	nodeAssert.deepEqual(ids(moveEntry(doc, 'cycles', { section: 0, index: 0 })), [
		['cycles', 'issues', 'board'],
		['forms']
	]);
	nodeAssert.deepEqual(ids(moveEntry(doc, 'board', { section: 1, index: 1 })), [
		['issues', 'cycles'],
		['forms', 'board']
	]);
	nodeAssert.deepEqual(ids(moveEntry(doc, 'nope', { section: 0, index: 0 })), ids(doc));
});

test('a step moves one place and crosses into the next section at the edge', () => {
	nodeAssert.deepEqual(ids(stepEntry(doc, 'board', -1)), [
		['board', 'issues', 'cycles'],
		['forms']
	]);
	nodeAssert.deepEqual(ids(stepEntry(doc, 'cycles', 1)), [
		['issues', 'board'],
		['cycles', 'forms']
	]);
	nodeAssert.deepEqual(ids(stepEntry(doc, 'forms', -1)), [
		['issues', 'board', 'cycles', 'forms'],
		[]
	]);
	nodeAssert.deepEqual(ids(stepEntry(doc, 'issues', -1)), ids(doc));
	nodeAssert.deepEqual(
		doc.sections[0].entries.map((e) => e.id),
		['issues', 'board', 'cycles']
	);
});

test('sections are added, ordered and removed only when empty', () => {
	const added = addSection(doc, { en: 'Ops', zh: '运维' });
	nodeAssert.equal(added?.sections[2].id, 'ops');
	nodeAssert.deepEqual(
		stepSection(doc, 'more', -1).sections.map((s) => s.id),
		['more', 'main']
	);
	nodeAssert.equal(removeSection(doc, 'main').sections.length, 2);
	nodeAssert.equal(removeSection(added as NavDoc, 'ops').sections.length, 2);
});

test('ids stay unique across sections and entries', () => {
	nodeAssert.equal(uniqueId(doc, 'More'), 'more-2');
	nodeAssert.equal(uniqueId(doc, '日本語'), 'item');
	nodeAssert.equal(addEntry(doc, 'main', entry('board')), null);
	nodeAssert.equal(
		addEntry(doc, 'main', entry('desk', 'project.apps'))?.sections[0].entries.length,
		4
	);
});

test('a built-in is offered only while no entry uses it', () => {
	const free = freeBuiltins(doc, 'project');
	nodeAssert.ok(!free.includes('project.issues'));
	nodeAssert.ok(free.includes('project.settings'));
	nodeAssert.ok(freeBuiltins(doc, 'project', 'project.issues').includes('project.issues'));
});

test('external links need http or https and no userinfo', () => {
	nodeAssert.ok(externalUrlOk('https://wiki.example.com/x'));
	nodeAssert.ok(externalUrlOk('http://intranet.local:8080/'));
	for (const bad of [
		'javascript:alert(1)',
		'ftp://x.test/',
		'https://u:p@x.test/',
		'x.test',
		' https://x.test/'
	])
		nodeAssert.ok(!externalUrlOk(bad), bad);
});

test('empty label texts are dropped', () => {
	nodeAssert.deepEqual(cleanLabels({ en: ' Intake ', zh: '' }), { en: 'Intake' });
	nodeAssert.equal(cleanLabels({ en: ' ', zh: '' }), null);
});
