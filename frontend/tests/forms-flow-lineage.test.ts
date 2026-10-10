/**
 * The Forms record detail's Flow section (`flow_lineage` on `GET /form-records/{record_id}`).
 *
 * Judged against `versions/v0.7-forms-bridge.md` "Web 交互" (lineage is visible from the Forms
 * end too; removing a reference never deletes the record) and the record-detail lineage rule that
 * a source the caller cannot read is reported as `{ id, source_available: false }` only. Covers:
 *
 * - visibility: field missing (older API), both lists empty (Flow/bridge off), one list populated;
 * - an unreadable source renders no link and nothing but the placeholder, also when the payload
 *   is malformed (`source_available: true` without an object id);
 * - link targets for the source Page and for the conversion job, the job link only with
 *   `job_available: true`;
 * - the page wiring: the section is driven by `flowLineageView`, its unavailable branches carry
 *   no `href`, it offers no remove action, and the page does not import `lib/api/flow.ts`;
 * - every mode/status key the view model can produce exists in both locales.
 *
 * Run standalone: `bun tests/forms-flow-lineage.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { withoutComments } from './support/source';
import { flowLineageView } from '../src/lib/forms/flow-lineage';
import type { FormRecordFlowLineage } from '../src/lib/api/forms';

const ROOT = new URL('..', import.meta.url).pathname;
const PAGE = 'obj-page-1';

const suite = new Suite('forms-flow-lineage');

const readable: FormRecordFlowLineage = {
	references: [
		{
			reference_id: 'ref-1',
			source_available: true,
			source_object_id: PAGE,
			source_title: 'Launch plan',
			display: { mode: 'reference' },
			created_at: '2026-10-01T08:00:00Z'
		},
		{
			reference_id: 'ref-2',
			source_available: true,
			source_object_id: 'obj-page-2',
			display: { mode: 'embed' },
			created_at: '2026-10-02T08:00:00Z'
		}
	],
	conversions: [
		{
			job_id: 'job-1',
			source_available: true,
			source_object_id: PAGE,
			source_title: 'Launch plan',
			job_available: true,
			status: 'completed',
			lineage_id: 'lin-1',
			created_at: '2026-10-03T08:00:00Z'
		},
		{
			job_id: 'job-2',
			source_available: true,
			source_object_id: PAGE,
			source_title: 'Launch plan',
			job_available: false,
			status: 'failed',
			lineage_id: null,
			created_at: '2026-10-04T08:00:00Z'
		}
	]
};

const hidden: FormRecordFlowLineage = {
	references: [{ reference_id: 'ref-hidden', source_available: false }],
	conversions: [{ job_id: 'job-hidden', source_available: false }]
};

// ---- 1. visibility -------------------------------------------------------------------------

suite.check('no flow_lineage field (older API) hides the section', () => {
	assertEqual(flowLineageView(undefined), null, 'undefined');
	assertEqual(flowLineageView(null), null, 'null');
});

suite.check('both lists empty (Flow or bridge off) hides the section', () => {
	assertEqual(flowLineageView({ references: [], conversions: [] }), null, 'empty lists');
});

suite.check('one populated list shows the section with the other list empty', () => {
	const onlyRefs = flowLineageView({ references: readable.references, conversions: [] });
	assert(onlyRefs, 'section shown with references only');
	assertEqual(onlyRefs.references.length, 2, 'two reference rows');
	assertEqual(onlyRefs.conversions.length, 0, 'conversions empty (page shows the empty copy)');
	const onlyJobs = flowLineageView({ references: [], conversions: readable.conversions });
	assert(onlyJobs, 'section shown with conversions only');
	assertEqual(onlyJobs.references.length, 0, 'references empty');
	assertEqual(onlyJobs.conversions.length, 2, 'two conversion rows');
});

// ---- 2. readable sources: link paths ---------------------------------------------------------

suite.check('a readable reference links to its Page and carries mode and time', () => {
	const view = flowLineageView(readable);
	assert(view, 'view');
	assertDeepEqual(
		view.references[0],
		{
			key: 'ref-1',
			available: true,
			sourceObjectId: PAGE,
			sourceTitle: 'Launch plan',
			mode: 'reference',
			createdAt: '2026-10-01T08:00:00Z'
		},
		'reference row'
	);
	assertDeepEqual(
		view.references[1],
		{
			key: 'ref-2',
			available: true,
			sourceObjectId: 'obj-page-2',
			sourceTitle: null,
			mode: 'embed',
			createdAt: '2026-10-02T08:00:00Z'
		},
		'untitled embed row (no projection title)'
	);
});

suite.check(
	'an unknown display mode falls back to "unknown" rather than echoing server data',
	() => {
		const view = flowLineageView({
			references: [
				{
					reference_id: 'ref-x',
					source_available: true,
					source_object_id: PAGE,
					display: { mode: 'card' },
					created_at: '2026-10-01T08:00:00Z'
				}
			],
			conversions: []
		});
		assert(view && view.references[0].available, 'available row');
		assertEqual(view.references[0].available && view.references[0].mode, 'unknown', 'mode');
	}
);

suite.check('a conversion links to its source Page, and to its job only when job_available', () => {
	const view = flowLineageView(readable);
	assert(view, 'view');
	assertDeepEqual(
		view.conversions[0],
		{
			key: 'job-1',
			available: true,
			sourceObjectId: PAGE,
			sourceTitle: 'Launch plan',
			status: 'completed',
			linkedJobId: 'job-1',
			createdAt: '2026-10-03T08:00:00Z'
		},
		'job the caller may open'
	);
	const restricted = view.conversions[1];
	assert(restricted.available, 'second row available');
	assertEqual(restricted.linkedJobId, null, 'job_available:false -> no job link');
	assertEqual(restricted.status, 'failed', 'status still shown');
});

// ---- 3. unreadable sources: no link, nothing but the placeholder -----------------------------

suite.check(
	'source_available:false rows carry only their key -- no link, title, mode, status, time',
	() => {
		const view = flowLineageView(hidden);
		assert(view, 'section shown (the record IS referenced, the caller just cannot see by whom)');
		assertDeepEqual(view.references, [{ key: 'ref-hidden', available: false }], 'reference row');
		assertDeepEqual(view.conversions, [{ key: 'job-hidden', available: false }], 'conversion row');
		const text = JSON.stringify(view);
		assert(
			!/sourceObjectId|linkedJobId|sourceTitle/.test(text),
			`no linkable id or title in ${text}`
		);
	}
);

suite.check('extra fields on an unavailable entry are ignored, not rendered', () => {
	const leaky = {
		references: [
			{
				reference_id: 'ref-leak',
				source_available: false,
				source_object_id: 'secret-page',
				source_title: 'Secret title',
				display: { mode: 'reference' },
				created_at: '2026-10-01T08:00:00Z'
			}
		],
		conversions: [
			{
				job_id: 'job-leak',
				source_available: false,
				source_object_id: 'secret-page',
				source_title: 'Secret title',
				job_available: true,
				status: 'completed'
			}
		]
	} as unknown as FormRecordFlowLineage;
	const view = flowLineageView(leaky);
	assert(view, 'view');
	const text = JSON.stringify(view);
	assert(!text.includes('secret-page') && !text.includes('Secret title'), `nothing leaks: ${text}`);
	assertDeepEqual(view.references[0], { key: 'ref-leak', available: false }, 'reference');
	assertDeepEqual(view.conversions[0], { key: 'job-leak', available: false }, 'conversion');
});

suite.check('source_available:true without an object id is treated as unavailable', () => {
	const malformed = {
		references: [{ reference_id: 'ref-m', source_available: true, display: {}, created_at: 'x' }],
		conversions: [{ job_id: 'job-m', source_available: true, status: 'completed', created_at: 'x' }]
	} as unknown as FormRecordFlowLineage;
	const view = flowLineageView(malformed);
	assert(view, 'view');
	assertDeepEqual(view.references[0], { key: 'ref-m', available: false }, 'reference');
	assertDeepEqual(view.conversions[0], { key: 'job-m', available: false }, 'conversion');
});

// ---- 4. page wiring --------------------------------------------------------------------------

const pagePath = join(
	ROOT,
	'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]/+page.svelte'
);
const pageSource = withoutComments(readFileSync(pagePath, 'utf8'));

function lineageBlock(): string {
	const start = pageSource.indexOf('data-testid="record-flow-lineage"');
	assert(start >= 0, 'the page renders a record-flow-lineage section');
	const open = pageSource.lastIndexOf('<details', start);
	const end = pageSource.indexOf('</details>', start);
	assert(open >= 0 && end > start, 'section is one <details> element');
	return pageSource.slice(open, end);
}

suite.check('the section is driven by flowLineageView and hidden when it returns null', () => {
	assert(
		/flowLineageView\(\s*record\.flow_lineage\s*\)/.test(pageSource),
		'page derives the view from record.flow_lineage'
	);
	assert(
		/\{#if record && flowLineage\}/.test(pageSource),
		'section is behind {#if ... flowLineage}'
	);
});

suite.check('the unavailable branches render the placeholder and no href', () => {
	const block = lineageBlock();
	const branches = [...block.matchAll(/\{:else\}([\s\S]*?)\{\/if\}/g)].map((match) => match[1]);
	const placeholders = branches.filter((body) => body.includes('record-flow-lineage-unavailable'));
	assertEqual(placeholders.length, 2, 'one placeholder branch for references, one for conversions');
	for (const body of placeholders) {
		assert(!/href|<a\b/.test(body), `placeholder branch renders a link:\n${body}`);
		assert(
			!/row\.(source|mode|status|created|job)/i.test(body),
			`placeholder shows row data:\n${body}`
		);
		assert(body.includes('forms.flowLineage.sourceUnavailable'), 'placeholder copy key');
	}
	const links = [...block.matchAll(/href=\{resolve\(\s*'([^']+)',\s*\{([^}]*)\}\s*\)\}/g)].map(
		(match) => `${match[1]} ${match[2].replace(/\s+/g, ' ').trim()}`
	);
	assertDeepEqual(
		links.sort(),
		[
			'/(app)/workspace/[workspaceId]/flow/[objectId] workspaceId, objectId: row.sourceObjectId',
			'/(app)/workspace/[workspaceId]/flow/[objectId] workspaceId, objectId: row.sourceObjectId',
			'/(app)/workspace/[workspaceId]/flow/conversions/[jobId] workspaceId, jobId: row.linkedJobId'
		],
		'source links go to the Page, the job link to the conversion job page'
	);
	assertEqual((block.match(/href=/g) ?? []).length, 3, 'no other link in the section');
	assert(/\{#if row\.linkedJobId\}/.test(block), 'job link only when linkedJobId is set');
});

suite.check('the section offers no remove action (removal lives on the Page card)', () => {
	const block = lineageBlock();
	assert(
		!/onclick|<button|<Button|delete|remove/i.test(block.replace(/readOnlyNote/g, '')),
		'no action in section'
	);
});

suite.check('the page does not import lib/api/flow.ts', () => {
	assert(!/\$lib\/api\/flow['"]/.test(pageSource), 'no flow api import');
	const model = readFileSync(join(ROOT, 'src/lib/forms/flow-lineage.ts'), 'utf8');
	assert(!/api\/flow['"]/.test(model), 'view model does not import the flow api either');
});

// ---- 5. copy ---------------------------------------------------------------------------------

suite.check('every forms.flowLineage key the page uses exists in zh and en', () => {
	const used = new Set(
		[...pageSource.matchAll(/'forms\.flowLineage\.([A-Za-z.]+)'/g)].map((match) => match[1])
	);
	for (const mode of ['reference', 'embed', 'unknown']) used.add(`mode.${mode}`);
	for (const status of ['started', 'completed', 'failed', 'unknown']) used.add(`status.${status}`);
	assert(used.size > 10, `found ${used.size} keys`);
	for (const locale of ['zh', 'en']) {
		const catalogue = JSON.parse(
			readFileSync(join(ROOT, 'src/lib/i18n', `${locale}.json`), 'utf8')
		);
		const lineage = catalogue.forms?.flowLineage;
		for (const key of used) {
			const value = key.split('.').reduce((node: unknown, part) => {
				return node && typeof node === 'object'
					? (node as Record<string, unknown>)[part]
					: undefined;
			}, lineage);
			assert(
				typeof value === 'string' && value.trim().length > 0,
				`${locale}.json forms.flowLineage.${key}`
			);
		}
	}
});

finish(suite.result());
