/**
 * No person is ever shown by a piece of their id: the `substring(0, 8)` / `slice(0, 8)` fallbacks
 * are gone from `src`. The few that remain cut something that is not a person (a record, a job,
 * a plugin template, a list limit) and are listed here with the reason. A work item is shown by its
 * identifier (`KEY-n`), never by a piece of its id.
 *
 * Run standalone: `bun test tests/features/people-no-id-fallbacks.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const SRC = new URL('../../src', import.meta.url).pathname;
const CUT = /\b(?:substring|slice)\(\s*0\s*,\s*8\s*\)/;

// path (relative to src) -> what the cut is of.
const NOT_PEOPLE: Record<string, string> = {
	'lib/forms/state/view-draft.ts': 'list limit',
	'lib/forms/view-arrangement.ts': 'random number from a generated id',
	'lib/forms/state/filter-expression-export.ts': 'list limit',
	'lib/forms/state/view-cards.ts': 'form record id',
	'lib/forms/state/view-timeline.ts': 'form record id',
	'lib/plugins/creator/ui-view-template.html': 'uuid formatting in a template',
	'lib/plugins/creator/order-amount-template.html': 'uuid formatting in a template',
	'lib/shell/column/lists.ts': 'list limit',
	'lib/project/connection/options.ts': 'project id used in a generated name',
	'lib/components/forms/RecordSidePanel.svelte': 'form record id',
	'lib/components/forms/RecordGrid.svelte': 'form record id',
	'lib/components/forms/RecordPanelSurface.svelte': 'form record id',
	'lib/components/forms/views/CalendarView.svelte': 'form record id',
	'lib/components/flow/FlowContextPanel.svelte': 'page id',
	'lib/components/flow/PortableGraphPackage.svelte': 'page and form ids in a package label',
	'lib/components/flow/collection/CollectionCellValue.svelte': 'record id of a relation',
	'lib/flow/package-export.ts': 'job id in a file name',
	'lib/flow/object-export.ts': 'job id in a file name',
	'routes/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]/+page.svelte':
		'list limit of the mention picker',
	'routes/(app)/trust-board/+page.svelte': 'hash shortening',
	'routes/(app)/workspace/[workspaceId]/webhooks/+page.svelte': 'delivery id',
	'routes/(app)/proposals/[id]/+page.svelte': 'list limit of the mention picker'
};

function files(dir: string, found: string[] = []): string[] {
	for (const entry of readdirSync(dir)) {
		const path = join(dir, entry);
		if (statSync(path).isDirectory()) files(path, found);
		else if (/\.(ts|svelte|html|js)$/.test(entry)) found.push(path);
	}
	return found;
}

test('no 8-character id cut is left except the ones that are not people', () => {
	const unexpected: string[] = [];
	const seen = new Set<string>();
	for (const path of files(SRC)) {
		const name = relative(SRC, path).split(sep).join('/');
		const lines = readFileSync(path, 'utf8').split('\n');
		lines.forEach((line, index) => {
			if (!CUT.test(line)) return;
			seen.add(name);
			if (!(name in NOT_PEOPLE)) unexpected.push(`${name}:${index + 1}: ${line.trim()}`);
		});
	}
	assert.deepEqual(unexpected, []);
	// Keep the list honest: a file that no longer has a cut must leave it.
	const stale = Object.keys(NOT_PEOPLE).filter((name) => !seen.has(name));
	assert.deepEqual(stale, []);
});

test('the lines that remain never cut a person, a member, an author, an actor or a bot', () => {
	const people =
		/\b(user|member|assignee|author|actor|participant|bot|person|owner|voter)[A-Za-z_]*\??\.(?:substring|slice)\(\s*0\s*,\s*8\s*\)/i;
	const hits: string[] = [];
	for (const path of files(SRC)) {
		const name = relative(SRC, path).split(sep).join('/');
		readFileSync(path, 'utf8')
			.split('\n')
			.forEach((line, index) => {
				if (people.test(line)) hits.push(`${name}:${index + 1}: ${line.trim()}`);
			});
	}
	assert.deepEqual(hits, []);
});

test('no person fallback falls back to an id field', () => {
	const idFallback =
		/(?:name|label|Name)\s*(?:\|\||\?\?)\s*[A-Za-z_.?]*(?:user_id|author_id|member_id|bot_id|actor_id|participant_id)\b/;
	const hits: string[] = [];
	for (const path of files(SRC)) {
		const name = relative(SRC, path).split(sep).join('/');
		readFileSync(path, 'utf8')
			.split('\n')
			.forEach((line, index) => {
				if (idFallback.test(line)) hits.push(`${name}:${index + 1}: ${line.trim()}`);
			});
	}
	assert.deepEqual(hits, []);
});
