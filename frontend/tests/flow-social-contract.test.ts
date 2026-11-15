// Developer transport contracts only; backend resource authorization is not mocked acceptance.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { flowApi } from '../src/lib/api/flow';

test('social/appearance/reference/inheritance calls use canonical object routes and exact scope-native payloads', async () => {
	const saved = globalThis.fetch;
	const calls: Array<{ url: string; method: string; body: unknown; key: string | null }> = [];
	let response: unknown = {};
	globalThis.fetch = (async (url, init) => {
		calls.push({
			url: String(url),
			method: init?.method ?? 'GET',
			body: init?.body ? JSON.parse(String(init.body)) : null,
			key: new Headers(init?.headers).get('Idempotency-Key')
		});
		return new Response(JSON.stringify({ code: 0, message: 'ok', data: response }), {
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	try {
		const body = { body: 'A comment', block_id: 'cid:1@71001:Map', mentions: ['recipient'] };
		response = {
			id: 'comment',
			object_id: 'object',
			...body,
			author_id: 'author',
			created_at: 'now',
			updated_at: 'now',
			parent_id: null,
			resolved_at: null,
			resolved_by: null
		};
		assert.deepEqual((await flowApi.createComment('object', body, 'create-intent')).data, response);
		assert.deepEqual(calls.at(-1), {
			url: '/api/v2/flow/objects/object/comments',
			method: 'POST',
			body,
			key: 'create-intent'
		});
		await flowApi.updateComment('object', 'comment', { body: 'Edited' }, 'edit-intent');
		assert.equal(calls.at(-1)?.key, 'edit-intent');
		await flowApi.resolveComment('object', 'comment', true, 'resolve-intent');
		assert.deepEqual(calls.at(-1)?.body, { resolved: true });
		await flowApi.deleteComment('object', 'comment', 'delete-intent');
		assert.equal(calls.at(-1)?.method, 'DELETE');
		assert.equal(calls.at(-1)?.key, 'delete-intent');
		const appearance = {
			icon: { kind: 'emoji' as const, emoji: '🌿' },
			cover: { kind: 'gallery' as const, gallery_id: 'moss', position: 0.5 }
		};
		response = appearance;
		assert.deepEqual(
			(await flowApi.setAppearance('object', appearance, 'appearance-intent')).data,
			appearance
		);
		await flowApi.uploadAppearance('object', 'synthetic-base64', 'upload-intent');
		assert.deepEqual(calls.at(-1)?.body, { data_base64: 'synthetic-base64' });
		await flowApi.referenceObject('object', {
			target_type: 'form',
			target_id: 'real-form',
			display: { label: 'Reference' },
			idempotency_key: 'ref-intent'
		});
		assert.equal(calls.at(-1)?.url, '/api/v2/flow/objects/object/references');
		await flowApi.unreferenceObject('object', 'reference', 'remove-intent');
		assert.equal(calls.at(-1)?.key, 'remove-intent');
		const inheritance = {
			inherit_from_parent: false,
			confirm_self_lockout: false,
			dry_run: true,
			initial_grants: [],
			idempotency_key: 'policy-intent'
		};
		await flowApi.setInheritance('object', inheritance);
		assert.deepEqual(calls.at(-1)?.body, inheritance);
		const link = {
			target_object_id: 'target-object',
			relation_type: 'reference',
			properties: { label: 'Related' }
		};
		await flowApi.linkObjects('object', link, 'link-intent');
		assert.deepEqual(calls.at(-1)?.body, {
			command: { type: 'link', payload: link },
			idempotency_key: 'link-intent'
		});
		await flowApi.unlinkObject('object', 'relation', 'unlink-intent');
		assert.deepEqual(calls.at(-1)?.body, {
			command: { type: 'unlink', payload: { relation_id: 'relation' } },
			idempotency_key: 'unlink-intent'
		});
		response = { items: [{ visibility: 'unavailable' }] };
		assert.deepEqual((await flowApi.getRelations('object', { direction: 'both' })).data, response);
		assert(calls.every((call) => call.url.startsWith('/api/v2/flow/objects/object/')));
		assert(calls.every((call) => !JSON.stringify(call.body).includes('workspace_id')));
	} finally {
		globalThis.fetch = saved;
	}
});

test('public share contract preserves opaque unavailable states and encodes the token as one path segment', async () => {
	const saved = globalThis.fetch;
	const urls: string[] = [];
	globalThis.fetch = (async (url) => {
		urls.push(String(url));
		return new Response(
			JSON.stringify({ code: 404, message: 'unavailable', error_code: 'not_found', data: null }),
			{ headers: { 'Content-Type': 'application/json' } }
		);
	}) as typeof fetch;
	try {
		const result = await flowApi.getPublicShare('synthetic/token?value');
		assert.equal(urls[0], '/api/v2/flow/public-shares/synthetic%2Ftoken%3Fvalue');
		assert.equal(result.error_code, 'not_found');
		assert.equal(result.data, null);
	} finally {
		globalThis.fetch = saved;
	}
});

test('scope package preview passes verified source/history and separate mappings through unchanged', async () => {
	const saved = globalThis.fetch;
	let request: { url: string; body: unknown } | null = null;
	const data = {
		preview_id: 'preview',
		package_id: 'package',
		package_sha256: 'hash',
		mapping_hash: 'mapping',
		manifest_summary: {
			projects: [],
			object_count: 1,
			document_count: 1,
			package_schema: 'sylvode.flow.export-package.v2',
			engine: { name: 'loro', crate_version: '1.13.9', wire_format_version: 1 },
			source: { scope_id: 'source-scope', scope: 'scope', root_object_ids: ['source-object'] },
			history: { included: false, snapshot_history: 'retained' }
		},
		mapping: {
			workspace_map: {},
			target_scope_id: 'personal-target',
			scope_map: { 'source-scope': 'personal-target' }
		},
		conflicts: [],
		conflict_details: [],
		warnings: [],
		estimated_changes: {},
		expires_at: 'later'
	};
	let wire: unknown = data;
	globalThis.fetch = (async (url, init) => {
		request = { url: String(url), body: JSON.parse(String(init?.body)) };
		return new Response(JSON.stringify({ code: 0, message: 'ok', data: wire }), {
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	try {
		const body = {
			artifact_id: 'artifact',
			project_mapping: {},
			external_reference_policy: 'reject' as const,
			conflict_policy: 'reject_existing' as const,
			include_history: false,
			idempotency_key: 'preview-intent'
		};
		const result = await flowApi.previewPackageImport('personal-target', body);
		assert.deepEqual(result.data, data);
		assert.deepEqual(request, { url: '/api/v2/flow/scopes/personal-target/imports/preview', body });
		assert.deepEqual(result.data?.mapping.workspace_map, {});
		assert.equal(result.data?.manifest_summary.history?.snapshot_history, 'retained');
		assert.equal(result.data?.manifest_summary.source?.workspace_id, undefined);
		wire = {
			...data,
			manifest_summary: {
				...data.manifest_summary,
				package_schema: 'sylvode.flow.export-package.v1',
				source: {
					workspace_id: 'genuine-source-workspace',
					scope: 'workspace',
					root_object_ids: ['source-object']
				}
			}
		};
		const legacy = await flowApi.previewPackageImport('personal-target', body);
		assert.equal(legacy.data?.manifest_summary.source?.scope_id, undefined);
		assert.equal(legacy.data?.manifest_summary.source?.workspace_id, 'genuine-source-workspace');
	} finally {
		globalThis.fetch = saved;
	}
});

test('share-link shapes stay object-scoped and denied appearance bytes remain a typed HTTP200 envelope', async () => {
	const saved = globalThis.fetch;
	const calls: Array<{ url: string; method: string; body: unknown }> = [];
	let denied = false;
	globalThis.fetch = (async (url, init) => {
		calls.push({
			url: String(url),
			method: init?.method ?? 'GET',
			body: init?.body ? JSON.parse(String(init.body)) : null
		});
		return new Response(
			JSON.stringify(
				denied
					? { code: 403, error_code: 'forbidden', message: 'denied', data: null }
					: { code: 0, message: 'ok', data: { items: [] } }
			),
			{ headers: { 'Content-Type': 'application/json' } }
		);
	}) as typeof fetch;
	try {
		await flowApi.listShareLinks('object');
		await flowApi.createShareLink('object', { expires_at: null });
		await flowApi.revokeShareLink('object', 'link');
		assert.deepEqual(
			calls.map((call) => [call.method, call.url, call.body]),
			[
				['GET', '/api/v2/flow/objects/object/share-links', null],
				['POST', '/api/v2/flow/objects/object/share-links', { expires_at: null }],
				['POST', '/api/v2/flow/objects/object/share-links/link/revoke', {}]
			]
		);
		denied = true;
		const bytes = await flowApi.appearanceAsset('object', 'asset');
		assert.equal(bytes.code, 403);
		assert.equal(bytes.error_code, 'forbidden');
		assert.equal(bytes.data, null);
	} finally {
		globalThis.fetch = saved;
	}
});
