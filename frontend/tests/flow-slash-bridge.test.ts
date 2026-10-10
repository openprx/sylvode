/**
 * Page slash menu: Collection embed and Forms Bridge Reference / Embed / Convert
 * (`ui-surface-v1.md` "后续版本 UI 派生" v0.6; `versions/v0.6-collections.md`;
 * `versions/v0.7-forms-bridge.md` "Web 交互"; `rest-api-v1.md` v0.6 / v0.7).
 *
 * - Collection: one `create_collection_embed` command through the real `CommandService` and a
 *   mocked `fetch` -- never `POST .../flow/objects` and never `insert_block`; one provisional
 *   block while pending; a failure removes it whole; retry resends the same key and body; a
 *   permanent failure offers no retry; classification never reads `message`;
 * - Reference / Embed: exact `POST .../references` bodies (`display.mode`), key reuse on retry;
 *   removal needs the user's confirmation and sends only `DELETE .../references/{id}`;
 * - Embed without `form.view` requests no Forms data at all;
 * - the Page placeholder block (real `loro-crdt` doc) carries only ids and the display mode;
 * - the card list merges document blocks, live references and the provisional block;
 * - the Forms picker requests each list only after its parent is chosen;
 * - adapter discipline: Flow components do not import `lib/api/flow.ts`.
 *
 * Run standalone: `bun tests/flow-slash-bridge.test.ts`
 */

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { LoroDoc } from 'loro-crdt';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { FlowCommandService } from '../src/lib/flow/command-service';
import {
	BridgeReferenceSession,
	CollectionEmbedSession,
	FORMS_EMBED_READS,
	FormsTargetPicker,
	PageBlockTree,
	REFERENCE_EMBED_TYPE,
	SLASH_BRIDGE_OPTIONS,
	buildPageCards,
	canViewTarget,
	convertWizardPath,
	embedResultIds,
	loadEmbedContent,
	permissionBadge,
	readPageBlocks,
	type AvailableReference,
	type CollectionEmbedSnapshot,
	type PageDocument,
	type ReferenceReads
} from '../src/lib/flow/slash-bridge';
import type { AcceptedChange, BridgePermissionState, FlowObjectView } from '../src/lib/api/flow';

const ROOT = new URL('..', import.meta.url).pathname;
const suite = new Suite('flow_slash_bridge');

// ---- fetch mock -------------------------------------------------------------------------------

interface Call {
	method: string;
	path: string;
	body: unknown;
	idempotencyKey: string | null;
}

type Responder = (call: Call) => { body: unknown } | 'throw';

const calls: Call[] = [];
let responder: Responder = () => ({ body: { code: 0, message: 'ok', data: {} } });

globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const url = raw.startsWith('http') ? new URL(raw) : new URL(raw, 'http://local');
	const call: Call = {
		method: init?.method ?? 'GET',
		path: url.pathname,
		body: init?.body ? JSON.parse(String(init.body)) : null,
		idempotencyKey: new Headers(init?.headers).get('Idempotency-Key')
	};
	calls.push(call);
	const answer = responder(call);
	if (answer === 'throw') throw new TypeError('network down');
	return new Response(JSON.stringify(answer.body), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

function reset(next: Responder): void {
	calls.length = 0;
	responder = next;
}

function ok(data: unknown): { body: unknown } {
	return { body: { code: 0, message: 'ok', data } };
}

function typedError(code: number, errorCode: string, message: string, details?: unknown) {
	return { body: { code, message, data: null, error_code: errorCode, details } };
}

const PAGE = '11111111-1111-4111-8111-111111111111';
const COLLECTION = '22222222-2222-4222-8222-222222222222';
const BLOCK = '33333333-3333-4333-8333-333333333333';

function page(): FlowObjectView {
	return {
		id: PAGE,
		workspace_id: 'ws',
		project_id: null,
		object_type: 'page',
		title: 'Page',
		parent_id: null,
		document_id: 'doc',
		document_seq: 3,
		frontier: 'f',
		projection_seq: 3,
		lifecycle_status: 'active',
		created_by: null,
		created_at: '2026-10-10T00:00:00Z',
		updated_at: '2026-10-10T00:00:00Z'
	} as unknown as FlowObjectView;
}

function accepted(commandResult: unknown): AcceptedChange {
	return {
		object: page(),
		accepted_seq: 4,
		head_frontier: 'f2',
		projection_seq: 4,
		semantic_diff: null,
		affected_object_ids: [PAGE, COLLECTION],
		event_id: 'event',
		command_result: commandResult
	};
}

const LABELS = { fieldLabel: 'Name', viewName: 'Table' };

let counter = 0;
const mint = (): string => `id-${(counter += 1)}`;

// ---- Collection embed -------------------------------------------------------------------------

await suite.checkAsync(
	'Collection sends exactly one create_collection_embed command, never createObject or insert_block',
	async () => {
		const seenDuringRequest: CollectionEmbedSnapshot[] = [];
		const snapshots: CollectionEmbedSnapshot[] = [];
		let session: CollectionEmbedSession | null = null;
		reset(() => {
			if (session) seenDuringRequest.push(session.snapshot());
			return ok(
				accepted({
					collection_id: COLLECTION,
					collection_document_id: 'cdoc',
					block_id: BLOCK
				})
			);
		});
		session = new CollectionEmbedSession(
			PAGE,
			new FlowCommandService(),
			(s) => snapshots.push(s),
			mint
		);
		const outcome = await session.start('  Roadmap  ', LABELS);
		assertEqual(outcome.kind, 'created', 'outcome');
		assertEqual(calls.length, 1, 'one request in total');
		const [call] = calls;
		assertEqual(call.method, 'POST', 'method');
		assertEqual(call.path, `/api/v1/flow/objects/${PAGE}/commands`, 'Page command endpoint');
		const body = call.body as {
			command: { type: string; payload: Record<string, unknown> };
			idempotency_key: string;
		};
		assertEqual(body.command.type, 'create_collection_embed', 'command type');
		assertEqual(body.command.payload.title, 'Roadmap', 'trimmed title');
		assert(typeof body.idempotency_key === 'string' && body.idempotency_key.length > 0, 'key');
		assert(
			!calls.some(
				(c) =>
					c.path.endsWith('/flow/objects') || JSON.stringify(c.body ?? '').includes('insert_block')
			),
			'no object create and no insert_block'
		);
		const fields = body.command.payload.initial_fields as Array<{ field_id: string }>;
		const view = body.command.payload.initial_view as { view_id: string; view_type: string };
		assertEqual(fields.length, 1, 'one initial field');
		assertEqual(view.view_type, 'table', 'initial table view');
		assertEqual(seenDuringRequest.length, 1, 'snapshot observed while the request was in flight');
		assertEqual(
			seenDuringRequest[0].provisional?.status,
			'pending',
			'one provisional block while pending'
		);
		assertEqual(seenDuringRequest[0].provisional?.title, 'Roadmap', 'provisional title');
		const after = session.snapshot();
		assertEqual(after.provisional?.status, 'accepted', 'accepted until the block arrives');
		assertEqual(after.provisional?.blockId, BLOCK, 'server block id');
		assertEqual(after.provisional?.collectionId, COLLECTION, 'server collection id');
		session.settle(new Set(['some-other-block']));
		assert(session.snapshot().provisional !== null, 'kept while the document lacks the block');
		session.settle(new Set([BLOCK]));
		assertEqual(session.snapshot().provisional, null, 'gone once the real block is in the doc');
	}
);

await suite.checkAsync(
	'failure removes the provisional block whole; retry resends the same key and body',
	async () => {
		let attempt = 0;
		reset(() => {
			attempt += 1;
			return attempt === 1
				? typedError(409, 'server_draining', 'please retry later', {
						reason: 'contention',
						retry_after_ms: 0
					})
				: ok(accepted({ collection_id: COLLECTION, block_id: BLOCK }));
		});
		const session = new CollectionEmbedSession(PAGE, new FlowCommandService(), () => {}, mint);
		const first = await session.start('Tasks', LABELS);
		assertEqual(first.kind, 'failed', 'first attempt fails');
		const failed = session.snapshot();
		assertEqual(failed.provisional, null, 'no provisional block after the failure');
		assertEqual(failed.failure?.code, 'server_draining', 'typed code');
		assertEqual(failed.failure?.retryable, true, 'contention is retryable');
		assertEqual(failed.failure?.key, 'flow.error.server_draining.contention', 'reason key');
		const keyBefore = session.idempotencyKey;
		const second = await session.retry();
		assertEqual(second.kind, 'created', 'retry succeeds');
		assertEqual(calls.length, 2, 'two requests');
		assertDeepEqual(calls[1].body, calls[0].body, 'identical body, key included');
		assertEqual(
			(calls[1].body as { idempotency_key: string }).idempotency_key,
			keyBefore,
			'retry key is the failed intent key'
		);
	}
);

await suite.checkAsync(
	'a transport failure is retryable with the generic key; a new start mints a new key',
	async () => {
		reset(() => 'throw');
		const session = new CollectionEmbedSession(PAGE, new FlowCommandService(), () => {}, mint);
		await session.start('Bugs', LABELS);
		const failure = session.snapshot().failure;
		assertEqual(failure?.retryable, true, 'retryable');
		assertEqual(failure?.key, 'flow.canvas.slashMenu.collectionFailed', 'generic key');
		const firstKey = (calls[0].body as { idempotency_key: string }).idempotency_key;
		await session.start('Bugs', LABELS);
		const secondKey = (calls[1].body as { idempotency_key: string }).idempotency_key;
		assert(firstKey !== secondKey, 'a fresh start is a new intent');
	}
);

await suite.checkAsync(
	'permanent failures offer no retry, and the message text is never consulted',
	async () => {
		for (const code of ['server_rejected', 'policy_rejected', 'invalid_update']) {
			reset(() => typedError(400, code, 'temporary glitch, please retry'));
			const session = new CollectionEmbedSession(PAGE, new FlowCommandService(), () => {}, mint);
			await session.start('X', LABELS);
			const failure = session.snapshot().failure;
			assertEqual(failure?.retryable, false, `${code} is permanent`);
			assertEqual(failure?.key, `flow.error.${code}`, `${code} key`);
			const again = await session.retry();
			assertEqual(again.kind, 'failed', 'retry refused');
			assertEqual(calls.length, 1, `${code}: retry sends nothing`);
		}
	}
);

await suite.checkAsync('an empty title is rejected before any request', async () => {
	reset(() => ok(accepted({})));
	const session = new CollectionEmbedSession(PAGE, new FlowCommandService(), () => {}, mint);
	const outcome = await session.start('   ', LABELS);
	assertEqual(outcome.kind, 'invalid', 'invalid');
	assertEqual(calls.length, 0, 'no request');
	assertEqual(session.snapshot().provisional, null, 'no provisional block');
});

suite.check('command_result ids are read in the server and the contract spelling', () => {
	assertDeepEqual(
		embedResultIds(accepted({ collection_id: 'c', block_id: 'b' })),
		{ collectionId: 'c', blockId: 'b' },
		'server spelling'
	);
	assertDeepEqual(
		embedResultIds(accepted({ created_collection_id: 'c', embed_block_id: 'b' })),
		{ collectionId: 'c', blockId: 'b' },
		'contract spelling'
	);
	assertDeepEqual(embedResultIds(accepted(null)), { collectionId: null, blockId: null }, 'missing');
});

// ---- Reference / Embed ------------------------------------------------------------------------

function permission(actions: string[]): BridgePermissionState {
	return {
		access: actions.some((a) => a !== 'form.view') ? 'controlled' : 'read_only',
		configuration: 'explicit',
		actions,
		field_read_limited: false,
		field_write_limited: false,
		record_limited: false
	};
}

function receipt(referenceId: string) {
	return ok({
		reference_id: referenceId,
		source_object_id: PAGE,
		target_type: 'form_record',
		target_id: 'rec-1',
		lineage_id: null,
		permission_state: permission(['form.view'])
	});
}

const noReads: ReferenceReads = {
	listReferences: async () => ({ code: 0, message: 'ok', data: { items: [] } })
};

await suite.checkAsync(
	'reference and embed requests carry the exact body and display.mode',
	async () => {
		reset(() => receipt('ref-1'));
		const session = new BridgeReferenceSession(PAGE, new FlowCommandService(), noReads, mint);
		const ref = await session.create({ targetType: 'form_record', targetId: 'rec-1' }, 'reference');
		assertEqual(ref.kind, 'created', 'reference created');
		const emb = await session.create({ targetType: 'form', targetId: 'form-1' }, 'embed');
		assertEqual(emb.kind, 'created', 'embed created');
		assertEqual(calls.length, 2, 'two requests');
		for (const call of calls) {
			assertEqual(call.method, 'POST', 'POST');
			assertEqual(call.path, `/api/v1/flow/objects/${PAGE}/references`, 'references endpoint');
		}
		const [first, second] = calls.map((c) => c.body as Record<string, unknown>);
		assertDeepEqual(
			Object.keys(first).sort(),
			['display', 'idempotency_key', 'target_id', 'target_type'],
			'no extra fields'
		);
		assertDeepEqual(
			{ ...first, idempotency_key: 'k' },
			{
				target_type: 'form_record',
				target_id: 'rec-1',
				display: { mode: 'reference' },
				idempotency_key: 'k'
			},
			'reference body'
		);
		assertDeepEqual(second.display, { mode: 'embed' }, 'embed display.mode');
		assertEqual(second.target_type, 'form', 'form target');
		assert(first.idempotency_key !== second.idempotency_key, 'distinct intents, distinct keys');
	}
);

await suite.checkAsync('a failed reference create retries with the same key', async () => {
	let attempt = 0;
	reset(() => {
		attempt += 1;
		return attempt === 1 ? 'throw' : receipt('ref-2');
	});
	const session = new BridgeReferenceSession(PAGE, new FlowCommandService(), noReads, mint);
	const target = { targetType: 'form_record' as const, targetId: 'rec-1' };
	const first = await session.create(target, 'reference');
	assertEqual(first.kind, 'failed', 'first fails');
	if (first.kind === 'failed') assertEqual(first.failure.retryable, true, 'retryable');
	const second = await session.create(target, 'reference');
	assertEqual(second.kind, 'created', 'second succeeds');
	assertEqual(
		(calls[1].body as { idempotency_key: string }).idempotency_key,
		(calls[0].body as { idempotency_key: string }).idempotency_key,
		'same key'
	);
});

await suite.checkAsync(
	'removal needs confirmation and only calls DELETE on the reference, never the target',
	async () => {
		reset(() => ok({ removed: true, event_id: 'ev' }));
		const session = new BridgeReferenceSession(PAGE, new FlowCommandService(), noReads, mint);
		const unconfirmed = await session.remove('ref-1', false);
		assertEqual(unconfirmed.kind, 'needs_confirmation', 'asks first');
		assertEqual(calls.length, 0, 'nothing sent without confirmation');
		const removed = await session.remove('ref-1', true);
		assertEqual(removed.kind, 'removed', 'removed');
		assertEqual(calls.length, 1, 'exactly one request');
		assertEqual(calls[0].method, 'DELETE', 'DELETE');
		assertEqual(
			calls[0].path,
			`/api/v1/flow/objects/${PAGE}/references/ref-1`,
			'the reference endpoint'
		);
		assert(calls[0].idempotencyKey, 'Idempotency-Key header');
		assert(
			!calls.some((c) => c.path.includes('/forms') || c.path.includes('/form-records')),
			'no Forms request'
		);
	}
);

await suite.checkAsync('a failed removal retries with the same key', async () => {
	let attempt = 0;
	reset(() => {
		attempt += 1;
		return attempt === 1 ? 'throw' : ok({ removed: true, event_id: 'ev' });
	});
	const session = new BridgeReferenceSession(PAGE, new FlowCommandService(), noReads, mint);
	await session.remove('ref-9', true);
	await session.remove('ref-9', true);
	assertEqual(calls[0].idempotencyKey, calls[1].idempotencyKey, 'same removal key');
});

function available(
	referenceId: string,
	targetType: 'form' | 'form_record',
	actions: string[],
	mode: 'reference' | 'embed' = 'embed'
): AvailableReference {
	return {
		visibility: 'available',
		reference_id: referenceId,
		source_object_id: PAGE,
		target_type: targetType,
		target_id: targetType === 'form' ? 'form-1' : 'rec-1',
		title: 'Target',
		status: 'active',
		summary: { secret: 'value' },
		display: { mode },
		permission_state: permission(actions)
	};
}

await suite.checkAsync('an embed without form.view requests no Forms data at all', async () => {
	reset(() => ok({}));
	for (const targetType of ['form', 'form_record'] as const) {
		const content = await loadEmbedContent(available('r', targetType, []), FORMS_EMBED_READS);
		assertEqual(content.kind, 'restricted', `${targetType}: placeholder`);
	}
	assertEqual(calls.length, 0, 'no request at all');
	assertEqual(canViewTarget(permission([])), false, 'no form.view, no view');
	assertEqual(permissionBadge(permission([])), 'no_access', 'badge');
});

await suite.checkAsync('an embed with form.view reads the record and its form', async () => {
	reset((call) =>
		call.path.startsWith('/api/v1/form-records/')
			? ok({
					id: 'rec-1',
					project_id: 'proj',
					form_id: 'form-1',
					title: 'Record',
					values: { note: 'hello', empty: '' }
				})
			: ok({
					id: 'form-1',
					project_id: 'proj',
					name: 'Plans',
					description: '',
					schema: { fields: [{ key: 'note', label: 'Note' }] }
				})
	);
	const content = await loadEmbedContent(
		available('r', 'form_record', ['form.view']),
		FORMS_EMBED_READS
	);
	assertDeepEqual(
		calls.map((c) => [c.method, c.path]),
		[
			['GET', '/api/v1/form-records/rec-1'],
			['GET', '/api/v1/forms/form-1']
		],
		'record then form'
	);
	assert(content.kind === 'record', 'record content');
	assertDeepEqual(content.fields, [{ label: 'Note', value: 'hello' }], 'labelled, empty dropped');
	assertEqual(content.projectId, 'proj', 'project for the source link');
	assertEqual(permissionBadge(permission(['form.view'])), 'read_only', 'view-only badge');
	assertEqual(
		permissionBadge(permission(['form.view', 'record.update'])),
		'controlled',
		'controlled badge'
	);
});

// ---- Page document blocks ---------------------------------------------------------------------

function serverPageDoc(): LoroDoc {
	const server = new LoroDoc();
	const tree = server.getTree('tree');
	tree.enableFractionalIndex(0);
	const node = tree.createNode(undefined);
	node.data.set('logical_id', BLOCK);
	node.data.set('kind', 'block');
	node.data.set('prop:embed_type', 'collection');
	node.data.set('prop:embed_object_id', COLLECTION);
	server.commit();
	return server;
}

suite.check('the server embed block is read from the Page tree', () => {
	const client = new LoroDoc();
	client.import(serverPageDoc().export({ mode: 'snapshot' }));
	assertDeepEqual(
		readPageBlocks(client.getTree('tree') as never),
		[{ kind: 'collection', blockId: BLOCK, collectionId: COLLECTION }],
		'collection embed block'
	);
});

suite.check('a reference placeholder carries only ids and the display mode', () => {
	const client = new LoroDoc();
	client.import(serverPageDoc().export({ mode: 'snapshot' }));
	const tree = new PageBlockTree(client as unknown as PageDocument, mint);
	assertEqual(tree.addReferencePlaceholder('ref-1', 'embed'), true, 'added');
	assertEqual(tree.addReferencePlaceholder('ref-1', 'embed'), false, 'idempotent');
	const nodes = client.getTree('tree').roots();
	assertEqual(nodes.length, 2, 'embed block + one placeholder');
	const placeholder = nodes[1].data.toJSON() as Record<string, unknown>;
	assertDeepEqual(
		Object.keys(placeholder).sort(),
		['kind', 'logical_id', 'prop:display_mode', 'prop:embed_type', 'prop:reference_id'],
		'no Forms content keys'
	);
	assertEqual(placeholder['prop:embed_type'], REFERENCE_EMBED_TYPE, 'embed type');
	assertEqual(placeholder['prop:reference_id'], 'ref-1', 'reference id');
	assertEqual(placeholder['prop:display_mode'], 'embed', 'mode');
	assertEqual(tree.removeReferencePlaceholders('ref-1'), 1, 'removed');
	assertEqual(client.getTree('tree').roots().length, 1, 'only the embed block left');
});

suite.check('cards follow document order; missing references are unavailable', () => {
	const blocks = [
		{ kind: 'collection' as const, blockId: 'b1', collectionId: 'c1' },
		{ kind: 'reference' as const, blockId: 'b2', referenceId: 'gone', mode: 'reference' as const },
		{ kind: 'reference' as const, blockId: 'b3', referenceId: 'r1', mode: 'embed' as const }
	];
	const cards = buildPageCards(
		blocks,
		[available('r1', 'form', ['form.view']), available('r2', 'form', [], 'reference')],
		{
			localId: 'p',
			title: 'New',
			status: 'pending',
			collectionId: null,
			blockId: null
		}
	);
	assertDeepEqual(
		cards.map((c) => c.kind),
		['collection', 'unavailable', 'reference', 'reference', 'provisional'],
		'order'
	);
	const settled = buildPageCards(blocks, [], {
		localId: 'p',
		title: 'New',
		status: 'accepted',
		collectionId: 'c1',
		blockId: 'b1'
	});
	assert(!settled.some((c) => c.kind === 'provisional'), 'no provisional once the block is there');
});

// ---- Forms picker -----------------------------------------------------------------------------

await suite.checkAsync(
	'the picker asks for each list only after its parent is chosen',
	async () => {
		const asked: string[] = [];
		const picker = new FormsTargetPicker('ws', {
			listProjects: async () => {
				asked.push('projects');
				return [{ id: 'p1', label: 'P1' }];
			},
			listForms: async (projectId) => {
				asked.push(`forms:${projectId}`);
				return {
					code: 0,
					message: 'ok',
					data: {
						items: [{ id: 'f1', name: 'F1' }],
						total: 1,
						page: 1,
						per_page: 100
					}
				} as never;
			},
			listRecords: async (formId) => {
				asked.push(`records:${formId}`);
				return {
					code: 0,
					message: 'ok',
					data: { items: [{ id: 'r1', title: 'R1' }], total: 1, page: 1, per_page: 50 }
				} as never;
			}
		});
		await picker.load();
		assertDeepEqual(asked, ['projects'], 'only projects at first');
		await picker.selectProject('p1');
		await picker.selectForm('f1');
		assertDeepEqual(
			picker.snapshot().target,
			{ targetType: 'form', targetId: 'f1' },
			'form target'
		);
		picker.selectRecord('r1');
		assertDeepEqual(
			picker.snapshot().target,
			{ targetType: 'form_record', targetId: 'r1' },
			'record target'
		);
		picker.selectRecord('not-listed');
		assertDeepEqual(picker.snapshot().target, { targetType: 'form', targetId: 'f1' }, 'unknown id');
		assertDeepEqual(asked, ['projects', 'forms:p1', 'records:f1'], 'request order');
	}
);

// ---- menu, convert entry, i18n, adapter discipline --------------------------------------------

function flatKeys(value: unknown, prefix = ''): Set<string> {
	const out = new Set<string>();
	if (value && typeof value === 'object') {
		for (const [key, child] of Object.entries(value)) {
			const path = prefix ? `${prefix}.${key}` : key;
			if (child && typeof child === 'object') for (const k of flatKeys(child, path)) out.add(k);
			else out.add(path);
		}
	}
	return out;
}

suite.check('the menu offers Collection, Reference, Embed and Convert, each with its note', () => {
	assertDeepEqual(
		SLASH_BRIDGE_OPTIONS.map((o) => o.action),
		['collection', 'reference', 'embed', 'convert'],
		'actions'
	);
	for (const locale of ['zh', 'en']) {
		const keys = flatKeys(
			JSON.parse(readFileSync(join(ROOT, `src/lib/i18n/${locale}.json`), 'utf8'))
		);
		for (const option of SLASH_BRIDGE_OPTIONS) {
			assert(keys.has(option.labelKey), `${locale}: ${option.labelKey}`);
			assert(keys.has(option.noteKey), `${locale}: ${option.noteKey}`);
		}
	}
	assertEqual(
		convertWizardPath('ws', PAGE),
		`/workspace/ws/flow/${PAGE}/convert`,
		'convert goes to the N4 wizard'
	);
});

suite.check('Flow components do not import lib/api/flow.ts', () => {
	const offenders: string[] = [];
	const walk = (dir: string): void => {
		for (const entry of readdirSync(dir)) {
			const full = join(dir, entry);
			if (statSync(full).isDirectory()) walk(full);
			else if (entry.endsWith('.svelte')) {
				const source = readFileSync(full, 'utf8');
				// Every import of the Flow REST module must be type-only (erased at build time).
				const imports = source.matchAll(
					/import\s+(type\s+)?[^;]*?from\s+['"]\$lib\/api\/flow['"]/g
				);
				for (const match of imports) if (!match[1]) offenders.push(full);
			}
		}
	};
	walk(join(ROOT, 'src/lib/components/flow'));
	assertDeepEqual(offenders, [], 'no component imports the Flow REST module');
});

finish(suite.result());
