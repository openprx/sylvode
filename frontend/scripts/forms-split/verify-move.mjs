#!/usr/bin/env node
// Checks that a wave-1 or wave-2 package of the forms page split only moved markup.
//
//   node scripts/forms-split/verify-move.mjs --base <commit> --package <id>
//
// (a) The route diff against <commit> consists only of the package's declared block
//     replacements (one component tag each) and its import slot line.
// (b) For every replaced block, the removed lines equal the component's markup after its
//     `</script>`, after inlining the package's child component tags recursively. Lines are
//     compared with leading whitespace removed and blank lines ignored.
//
// Wave 0 and W3 packages are skipped (the route changes arbitrarily there).
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = new URL('../../', import.meta.url).pathname;
const ROUTE = 'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/+page.svelte';
const PAGE_DIR = 'src/lib/components/forms/page';
const VIEWS_DIR = 'src/lib/components/forms/views';

// Declared tags per package. `route` tags replace a block in the route; `child` tags are
// replaced inside the new components and are inlined for check (b).
const PACKAGES = {
	'W1-01': { route: ['FormsPageHeader', 'FormsIndex'], child: [] },
	'W1-02': { route: ['FormCreatePanel'], child: [] },
	'W1-03': { route: ['FormDataToolbar', 'FormConfigHeader', 'PermissionStateSummary'], child: [] },
	'W1-04': { route: ['FormPermissionsMode'], child: [] },
	'W1-05': { route: ['FormImportExportMode', 'ImportWizardModal'], child: [] },
	'W1-06': { route: ['FormDesignMode', 'DesignerConfirmModal'], child: [] },
	'W1-07': {
		route: ['DetailLayoutMode'],
		child: ['DetailSectionsBuilder', 'DetailWidgetsBuilder']
	},
	'W1-08': {
		route: [
			'ViewSetupBasics',
			'ViewCardFieldOrder',
			'ViewKanbanWipPolicy',
			'ViewColumnsDisclosure'
		],
		child: []
	},
	'W1-09': { route: ['ViewFilterEditor'], child: [] },
	'W1-10': {
		route: ['ViewTypeOptions'],
		child: ['ViewPivotOptions', 'ViewGanttOptions', 'ViewCalendarOptions', 'ViewCardOptions']
	},
	'W1-11': { route: ['RecordEditorDrawer'], child: [] },
	'W1-12': { route: ['GanttView', 'DetailView'], child: [] },
	'W1-13': { route: ['PivotView'], child: [] },
	'W1-14': { route: ['KanbanView'], child: [] },
	'W1-15': { route: ['CalendarView'], child: [] },
	'W1-16': { route: ['CardView'], child: [] },
	'W1-17': { route: ['TimelineView'], child: [] },
	'W1-18': {
		route: ['RecordDetailAside'],
		child: ['RecordDetailLayout', 'RecordChildRecords', 'RecordChildEditor', 'RecordDetailActivity']
	},
	'W2-01': {
		route: ['FormWorkspace'],
		child: ['RecordListStack', 'RecordListPanel', 'LayoutSettingsContent']
	}
};

function arg(name) {
	const index = process.argv.indexOf(`--${name}`);
	return index >= 0 ? process.argv[index + 1] : undefined;
}

function fail(message) {
	console.error(`verify-move: FAIL ${message}`);
	process.exit(1);
}

const base = arg('base');
const id = arg('package');
if (!base || !id) fail('usage: verify-move.mjs --base <commit> --package <id>');
if (id.startsWith('W0') || id.startsWith('W3')) {
	console.log(`verify-move: ${id} skipped (wave 0 and W3 are not block moves)`);
	process.exit(0);
}
const declared = PACKAGES[id];
if (!declared) fail(`unknown package ${id}`);

function componentPath(name) {
	for (const dir of [PAGE_DIR, VIEWS_DIR]) {
		const path = join(root, dir, `${name}.svelte`);
		if (existsSync(path)) return path;
	}
	return null;
}

const norm = (lines) => lines.map((line) => line.trim()).filter((line) => line !== '');

function markupOf(name, seen = new Set()) {
	const path = componentPath(name);
	if (!path) fail(`component ${name} does not exist`);
	if (seen.has(name)) fail(`component ${name} includes itself`);
	const text = readFileSync(path, 'utf8');
	const end = text.indexOf('</script>');
	if (end < 0) fail(`${name} has no script block`);
	const markup = text.slice(end + '</script>'.length).split('\n');
	const out = [];
	for (const line of markup) {
		const tag = /^\s*<([A-Z]\w*) \/>\s*$/.exec(line);
		if (tag && declared.child.includes(tag[1])) {
			out.push(...markupOf(tag[1], new Set([...seen, name])));
		} else out.push(line);
	}
	return out;
}

const diff = execFileSync('git', ['diff', '-U0', '--no-color', base, '--', ROUTE], {
	cwd: root,
	encoding: 'utf8',
	maxBuffer: 256 * 1024 * 1024
});
const hunks = [];
for (const line of diff.split('\n')) {
	if (line.startsWith('@@')) hunks.push({ removed: [], added: [] });
	else if (hunks.length) {
		if (line.startsWith('-') && !line.startsWith('---')) hunks.at(-1).removed.push(line.slice(1));
		else if (line.startsWith('+') && !line.startsWith('+++'))
			hunks.at(-1).added.push(line.slice(1));
	}
}

const slotLine = `// forms-split slot ${id}`;
const seenTags = [];
let slotRemoved = false;
for (const [index, hunk] of hunks.entries()) {
	const where = `hunk ${index + 1}`;
	const slot = hunk.removed.findIndex((line) => line.trim() === slotLine);
	if (slot >= 0) {
		if (slotRemoved) fail(`${where}: slot line removed twice`);
		slotRemoved = true;
		if (hunk.removed.length !== 1) fail(`${where}: the slot hunk removes more than the slot line`);
		for (const line of hunk.added) {
			const match =
				/^\s*import\s.+from '#lib\/components\/forms\/(page|views)\/(\w+)\.svelte';\s*$/.exec(line);
			if (line.trim() !== '' && (!match || !declared.route.includes(match[2]))) {
				fail(`${where}: unexpected line in the slot: ${line.trim()}`);
			}
		}
		continue;
	}
	const tags = hunk.added
		.map((line) => /^\s*<([A-Z]\w*) \/>\s*$/.exec(line))
		.filter((match) => match !== null);
	if (hunk.added.length !== 1 || tags.length !== 1 || !declared.route.includes(tags[0][1])) {
		fail(`${where}: the added lines are not exactly one declared tag: ${hunk.added.join(' | ')}`);
	}
	const name = tags[0][1];
	seenTags.push(name);
	const expected = norm(markupOf(name));
	const actual = norm(hunk.removed);
	if (expected.length !== actual.length) {
		fail(
			`${where} (${name}): ${actual.length} removed lines, component markup has ${expected.length}`
		);
	}
	for (let i = 0; i < expected.length; i += 1) {
		if (expected[i] !== actual[i]) {
			fail(
				`${where} (${name}): line ${i + 1} differs\n  removed:   ${actual[i]}\n  component: ${expected[i]}`
			);
		}
	}
}
if (!slotRemoved) fail(`slot line "${slotLine}" was not replaced`);
for (const tag of declared.route) {
	const count = seenTags.filter((name) => name === tag).length;
	if (count !== 1) fail(`tag ${tag} replaces ${count} blocks, expected 1`);
}
console.log(`verify-move: ${id} ok (${seenTags.length} blocks, slot replaced)`);
