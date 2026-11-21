// The WP8 acceptance cases A01 to A20, acting as the client (Ramflux) against a live Sylvode.
//
// Each case is independent: it connects its own grant where the contract says "reconnect", or
// takes the shared one (reference scopes, projects {P1}) which is replaced whenever it is dead.
// Assertions throw messages built from redacted data only. See README.md.

import { test as base } from '@playwright/test';
import { CALLBACK_PATH, parseCallback } from './lib/callback.ts';
import { bodyErrorCode, challengeOf, challengeParam, expectBearerFailure } from './lib/bearer.ts';
import { runCase } from './lib/case.ts';
import { loadConfig } from './config.ts';
import { personalSettings, signIn, workspaceDisconnect } from './lib/consent.ts';
import { call } from './lib/http.ts';
import { mcpRpc, toolNames, toolOutcome } from './lib/mcp.ts';
import { redactor } from './lib/redact.ts';
import { exchangeCode, REFERENCE_SCOPE, discover, refreshGrant } from './lib/oauth-client.ts';
import { repoRootOf, runSpec, scriptExists, specsFor } from './lib/regression.ts';
import { Recorder } from './lib/record.ts';
import { asEvent, asControl, type EventStream, type SvEvent } from './lib/stream.ts';
import { itemsOf } from './lib/sv.ts';
import {
	asRecord,
	check,
	checkEqual,
	dig,
	mono,
	randomHex,
	sameSet,
	sleep,
	str
} from './lib/util.ts';
import { World, type CaseCtx } from './lib/world.ts';
import type { Connection } from './lib/oauth-client.ts';

const test = base.extend<object, { world: World }>({
	world: [
		// eslint-disable-next-line no-empty-pattern
		async ({}, use) => {
			const cfg = loadConfig(process.env, 'run');
			redactor.register(cfg.oprBotToken, 'bot-token');
			const world = await World.create(cfg);
			try {
				await use(world);
			} finally {
				await world.teardown();
			}
		},
		{ scope: 'worker' }
	]
});

test.describe.configure({ mode: 'default' });

const READ_TOOLS = [
	'projects.list',
	'projects.get',
	'context.get_project',
	'work_items.list',
	'work_items.get',
	'work_items.get_by_identifier',
	'work_items.list_labels',
	'comments.list'
];

// ---- helpers --------------------------------------------------------------------------------

function projectScope(world: World): { names: string[] } {
	return { names: [world.cfg.p1Name] };
}

function relatedIssue(event: SvEvent): string | undefined {
	return str(asRecord(event.subject.related)?.issue_id);
}

/** After a revocation: every credential of the grant is dead and the stream cannot be reopened. */
async function expectDead(c: CaseCtx, conn: Connection): Promise<void> {
	const apiToken = conn.lastHeld('api');
	const mcpToken = conn.lastHeld('mcp');
	check(apiToken !== undefined && mcpToken !== undefined, 'the connection holds no tokens to test');
	const reopened = await c.openStream(conn, 'reconnect after revocation', { token: apiToken });
	checkEqual(reopened.status, 401, 'reconnecting the stream after revocation');
	const connection = await call({
		rec: c.rec,
		label: 'API call after revocation',
		method: 'GET',
		url: c.world.delegatedUrl('/oauth/connection'),
		bearer: apiToken
	});
	expectBearerFailure(connection, 'API call after revocation', {
		status: 401,
		error: 'invalid_token'
	});
	const mcp = await mcpRpc(
		c.rec,
		c.world.env().disc.mcpResource,
		mcpToken,
		'MCP call after revocation',
		'tools/list'
	);
	checkEqual(mcp.status, 401, 'MCP call after revocation');
	const refreshed = await conn.rotate(c.rec, 'api', 'token: refresh after revocation');
	checkEqual(refreshed.res.status, 400, 'refresh after revocation (status)');
	checkEqual(refreshed.error, 'invalid_grant', 'refresh after revocation (error)');
}

/** Waits for the `revoked` control with `reason` and checks the 5 s limit from `t0`. */
async function expectRevoked(
	c: CaseCtx,
	stream: EventStream,
	t0: number,
	reason: string,
	timingName: string
): Promise<number> {
	const limit = c.world.cfg.limitMs;
	const arrived = await stream.waitForControl('revoked', limit + 5000);
	check(
		arrived !== null,
		`no revoked control message within ${limit + 5000} ms of the trigger (stream: ${stream.ended ? (stream.endReason ?? 'ended') : 'still open'})`
	);
	const control = asControl(arrived);
	checkEqual(control?.reason, reason, 'revoked reason');
	const elapsed = Math.max(0, arrived.t - t0);
	c.timing(timingName, elapsed);
	check(
		elapsed <= limit,
		`the revoked message took ${Math.round(elapsed)} ms, over the ${limit} ms limit`
	);
	check(await stream.untilEnded(5000), 'the stream stayed open after revoked');
	return stream.items.indexOf(arrived);
}

async function connectAndStream(
	c: CaseCtx
): Promise<{ conn: Connection; grantId: string; stream: EventStream }> {
	const connected = await c.world.connect(c, { projects: projectScope(c.world) });
	const stream = await c.openStreamOrFail(connected.conn, 'event stream');
	return { conn: connected.conn, grantId: connected.grantId, stream };
}

async function mcpToolCall(
	c: CaseCtx,
	token: string,
	label: string,
	name: string,
	args: Record<string, unknown>
) {
	return mcpRpc(c.rec, c.world.env().disc.mcpResource, token, label, 'tools/call', {
		name,
		arguments: args
	});
}

// ---- A01 to A06: connect and use ------------------------------------------------------------

test('A01 connect with the reference scopes and projects {P1}', async ({ browser, world }) => {
	await runCase(world, browser, 'A01', async (c) => {
		const disc = await discover(world.cfg, c.rec);
		check(disc.problems.length === 0, `discovery problems: ${disc.problems.join('; ')}`);
		const connected = await world.connect(c, { projects: projectScope(world) });
		const { mcpTokens, apiTokens } = connected;

		for (const [label, tokens] of [
			['MCP', mcpTokens],
			['API', apiTokens]
		] as const) {
			checkEqual(tokens.res.status, 200, `${label} token response status`);
			checkEqual(tokens.tokenType, 'Bearer', `${label} token type`);
			check(
				tokens.accessToken?.startsWith('sva_') === true,
				`${label} access token has the access-token prefix`
			);
			check(
				tokens.refreshToken?.startsWith('svr_') === true,
				`${label} response carries a refresh token with the refresh prefix`
			);
			check(
				(tokens.expiresIn ?? 0) >= 60 && (tokens.expiresIn ?? 0) <= 3600,
				`${label} expires_in is within 60..3600 (got ${tokens.expiresIn})`
			);
			check(
				(tokens.res.headers.get('cache-control') ?? '').includes('no-store'),
				`${label} token response has Cache-Control: no-store`
			);
			checkEqual(tokens.grantId, connected.grantId, `${label} grant id`);
			checkEqual(tokens.workspaceId, world.ids.workspaceId, `${label} workspace id`);
		}
		check(
			sameSet((mcpTokens.scope ?? '').split(' '), ['projects:read', 'work_items:read']),
			`MCP token scope is the grant intersected with the MCP resource (got "${mcpTokens.scope}")`
		);
		check(
			sameSet((apiTokens.scope ?? '').split(' '), [
				'projects:read',
				'work_items:read',
				'notifications:read',
				'tasks:read'
			]),
			`API token scope omits offline_access (got "${apiTokens.scope}")`
		);

		const token = await connected.conn.access(c.rec, 'api');
		const res = await call({
			rec: c.rec,
			label: 'GET connection',
			method: 'GET',
			url: world.delegatedUrl('/oauth/connection'),
			bearer: token
		});
		checkEqual(res.status, 200, 'GET /oauth/connection status');
		const data = asRecord(asRecord(res.json)?.data);
		check(data !== null, 'the connection answer has no data');
		checkEqual(str(data.grant_id), connected.grantId, 'connection grant id');
		checkEqual(str(data.client_id), world.cfg.clientId, 'connection client id');
		checkEqual(str(dig(data, 'workspace', 'id')), world.ids.workspaceId, 'connection workspace');
		checkEqual(str(dig(data, 'account', 'user_id')), world.ids.userAId, 'connection account');
		checkEqual(str(data.project_mode), 'selected', 'connection project mode');
		const scopes = (Array.isArray(data.scopes) ? data.scopes : []).map(String);
		check(
			sameSet(scopes, REFERENCE_SCOPE.split(' ')),
			'connection scopes are the requested reference set'
		);
		const projectIds = (Array.isArray(data.projects) ? data.projects : []).map(
			(p) => str(asRecord(p)?.id) ?? ''
		);
		check(sameSet(projectIds, [world.ids.p1Id]), 'the connection lists exactly project P1');
		c.detail('grant_id', connected.grantId);

		const context = await browser.newContext({
			locale: 'en-US',
			viewport: { width: 1280, height: 900 }
		});
		c.onCleanup('settings context', () => context.close());
		const page = await context.newPage();
		await signIn(page, world.cfg.uiOrigin, world.cfg.userA);
		const rows = await personalSettings(page, world.cfg.uiOrigin, {});
		check(
			rows.length === 1 && rows[0] === connected.grantId,
			`personal settings shows exactly the new connection (rows: ${rows.length})`
		);
		world.adoptShared(connected.conn);
	});
});

test('A02 tools/list with the MCP token', async ({ browser, world }) => {
	await runCase(world, browser, 'A02', async (c) => {
		const conn = await world.sharedConnection(c);
		const res = await mcpRpc(
			c.rec,
			world.env().disc.mcpResource,
			await conn.access(c.rec, 'mcp'),
			'MCP tools/list',
			'tools/list'
		);
		checkEqual(res.status, 200, 'tools/list status');
		const names = toolNames(res);
		check(names !== null, 'tools/list returned no tool array');
		c.detail('tools', [...names].sort());
		check(
			sameSet(names, READ_TOOLS) && names.length === READ_TOOLS.length,
			`the tool list is exactly the ${READ_TOOLS.length} read tools of projects:read and work_items:read`
		);
	});
});

test('A03 work_items.get in P1 and in P2', async ({ browser, world }) => {
	await runCase(world, browser, 'A03', async (c) => {
		const conn = await world.sharedConnection(c);
		const mcp = await conn.access(c.rec, 'mcp');
		const api = await conn.access(c.rec, 'api');
		const p1 = await world.seedItem(c.rec, 'p1');
		const p2 = await world.seedItem(c.rec, 'p2');
		const before = JSON.stringify(await world.getIssue(c.rec, p2.id));

		const okMcp = await mcpToolCall(c, mcp, 'MCP work_items.get (P1)', 'work_items.get', {
			work_item_id: p1.id
		});
		const okOutcome = toolOutcome(okMcp);
		check(
			okMcp.status === 200 && okOutcome.rpcError === null && !okOutcome.isError,
			`work_items.get on the P1 item failed (HTTP ${okMcp.status})`
		);
		check(okOutcome.text.includes(p1.id), 'the P1 answer does not describe the P1 item');

		const refused = await mcpToolCall(c, mcp, 'MCP work_items.get (P2)', 'work_items.get', {
			work_item_id: p2.id
		});
		const outcome = toolOutcome(refused);
		const refusedAsBuilt =
			refused.status === 403 ||
			(refused.status === 200 && (outcome.isError || outcome.rpcError !== null));
		check(refusedAsBuilt, `the P2 item was not refused through MCP (HTTP ${refused.status})`);
		check(!outcome.text.includes(p2.title), 'the refusal leaks the title of the P2 item');
		c.detail('mcp_p2_refusal', {
			http: refused.status,
			isError: outcome.isError,
			rpcError: outcome.rpcError
		});

		const viaApi = await call({
			rec: c.rec,
			label: 'API GET issue (P2)',
			method: 'GET',
			url: world.delegatedUrl(`/issues/${p2.id}`),
			bearer: api
		});
		checkEqual(viaApi.status, 403, 'API read of the P2 item');
		checkEqual(bodyErrorCode(viaApi), 'forbidden', 'API refusal error code');
		const okApi = await call({
			rec: c.rec,
			label: 'API GET issue (P1)',
			method: 'GET',
			url: world.delegatedUrl(`/issues/${p1.id}`),
			bearer: api
		});
		checkEqual(okApi.status, 200, 'API read of the P1 item');
		checkEqual(
			JSON.stringify(await world.getIssue(c.rec, p2.id)),
			before,
			'the P2 item after the refused reads'
		);
	});
});

test('A04 a write attempt', async ({ browser, world }) => {
	await runCase(world, browser, 'A04', async (c) => {
		const conn = await world.sharedConnection(c);
		const mcp = await conn.access(c.rec, 'mcp');
		const api = await conn.access(c.rec, 'api');
		const p1 = await world.seedItem(c.rec, 'p1');
		const before = JSON.stringify(await world.getIssue(c.rec, p1.id));
		const attempt = { title: 'qa-p4-must-not-apply' };

		// The write tools are withheld from every delegated caller in this release.
		const viaMcp = await mcpToolCall(c, mcp, 'MCP work_items.update', 'work_items.update', {
			work_item_id: p1.id,
			...attempt
		});
		checkEqual(viaMcp.status, 403, 'MCP write attempt status');
		const outcome = toolOutcome(viaMcp);
		checkEqual(outcome.rpcError?.code, -32002, 'MCP write attempt JSON-RPC code');
		checkEqual(
			outcome.rpcError?.message,
			'tool not available to connected applications',
			'MCP write attempt message'
		);
		checkEqual(
			viaMcp.headers.get('www-authenticate'),
			null,
			'MCP write attempt carries no scope challenge (no scope would help)'
		);

		// The API route exists for work_items:write, which this grant does not have.
		const viaApi = await call({
			rec: c.rec,
			label: 'API PUT issue',
			method: 'PUT',
			url: world.delegatedUrl(`/issues/${p1.id}`),
			bearer: api,
			json: attempt
		});
		expectBearerFailure(viaApi, 'API write attempt', { status: 403, error: 'insufficient_scope' });
		checkEqual(
			challengeParam(challengeOf(viaApi), 'scope'),
			'work_items:read work_items:write',
			'API write attempt challenge scope'
		);

		// The consent page cannot be asked for the write scope: the client is not allowed it.
		const started = await world.authorize(
			c,
			{ mode: 'deny' },
			{ scope: `${REFERENCE_SCOPE} work_items:write`, drive: false }
		);
		const target = await started.listener.waitForCallback(20_000);
		const verdict = parseCallback(target, {
			path: CALLBACK_PATH,
			iss: world.cfg.uiOrigin,
			state: started.state
		});
		check(
			!verdict.ok && verdict.kind === 'error' && verdict.error === 'invalid_scope',
			`asking for the write scope was not refused with invalid_scope (${JSON.stringify(redactor.json(verdict))})`
		);

		checkEqual(
			JSON.stringify(await world.getIssue(c.rec, p1.id)),
			before,
			'the P1 item after the write attempts'
		);
	});
});

test('A05 the MCP token presented to the API', async ({ browser, world }) => {
	await runCase(world, browser, 'A05', async (c) => {
		const conn = await world.sharedConnection(c);
		const res = await call({
			rec: c.rec,
			label: 'API call with the MCP token',
			method: 'GET',
			url: world.delegatedUrl('/oauth/connection'),
			bearer: await conn.access(c.rec, 'mcp')
		});
		expectBearerFailure(res, 'MCP token at the API', { status: 401, error: 'invalid_token' });
	});
});

test('A06 the API token presented to the MCP server', async ({ browser, world }) => {
	await runCase(world, browser, 'A06', async (c) => {
		const conn = await world.sharedConnection(c);
		const res = await mcpRpc(
			c.rec,
			world.env().disc.mcpResource,
			await conn.access(c.rec, 'api'),
			'MCP call with the API token',
			'tools/list'
		);
		checkEqual(res.status, 401, 'API token at the MCP server');
		checkEqual(challengeParam(challengeOf(res), 'error'), 'invalid_token', 'MCP challenge error');
		check(
			challengeParam(challengeOf(res), 'resource_metadata') !== undefined,
			'the MCP challenge names its resource metadata'
		);
	});
});

// ---- A07 to A10: the event stream -----------------------------------------------------------

test('A07 last hop: notification', async ({ browser, world }) => {
	await runCase(world, browser, 'A07', async (c) => {
		const conn = await world.sharedConnection(c);
		const stream = await c.openStreamOrFail(conn, 'event stream');
		const itemId = await world.createItem(c.rec, world.ids.p1Id, `qa-p4-a07-${randomHex(3)}`);
		const t0 = await world.assignToA(c.rec, itemId);
		const arrived = await stream.waitForEvent(
			(event) => event.type === 'notification.created' && relatedIssue(event) === itemId,
			world.cfg.limitMs + 5000
		);
		check(
			arrived !== null,
			`no notification.created for the assigned item within ${world.cfg.limitMs + 5000} ms`
		);
		const event = asEvent(arrived);
		const elapsed = Math.max(0, arrived.t - t0);
		c.timing('notification_ms', elapsed);
		check(
			elapsed <= world.cfg.limitMs,
			`the notification took ${Math.round(elapsed)} ms, over the ${world.cfg.limitMs} ms limit`
		);
		checkEqual(str(event?.envelope.spec), 'sylvode.delegated_event.v1', 'event spec');
		checkEqual(event?.workspaceId, world.ids.workspaceId, 'event workspace');
		checkEqual(event?.projectId, world.ids.p1Id, 'event project');
		check((event?.frameId ?? '') !== '', 'the event frame carries an id');
		c.detail(
			'trigger',
			'B assigned a new P1 item to A; the interval runs from the end of that response'
		);
	});
});

test('A08 last hop: task', async ({ browser, world }) => {
	await runCase(world, browser, 'A08', async (c) => {
		const conn = await world.sharedConnection(c);
		const stream = await c.openStreamOrFail(conn, 'event stream');
		const item = await world.seedItem(c.rec, 'p1');
		const created = await world.userB.request(
			c.rec,
			'B creates an AI task for the bot',
			'POST',
			`/api/v1/projects/${world.ids.p1Id}/ai/tasks`,
			{
				ai_participant_id: world.ids.botId,
				task_type: 'issue_assigned',
				reference_type: 'work_item',
				reference_id: item.id
			}
		);
		check(
			created.status === 200 && asRecord(created.json)?.code === 0,
			`task creation was refused (HTTP ${created.status}, ${str(asRecord(created.json)?.message) ?? ''})`
		);
		const taskId = str(asRecord(asRecord(created.json)?.data)?.id);
		check(taskId !== undefined, 'task creation returned no task id');
		c.onCleanup('end the task', async () => {
			// Stop the worker's retry cycle so later cases see no task events.
			await world.admin.request(
				new Recorder(),
				'end the task',
				'POST',
				`/api/v1/ai/callbacks/task/${taskId}/fail`,
				{ error_message: 'qa-p4 cleanup', retry: false }
			);
		});
		const t0 = created.doneAt;

		// The commit of the status change as a client can observe it: the task list read by polling.
		let seenProcessing: number | undefined;
		let polling = true;
		const poller = (async () => {
			while (polling) {
				const res = await world.userB
					.request(
						new Recorder(),
						'poll tasks',
						'GET',
						`/api/v1/projects/${world.ids.p1Id}/ai/tasks?per_page=50`
					)
					.catch(() => null);
				const seen =
					res === null
						? undefined
						: itemsOf(asRecord(res.json)?.data)
								.map(asRecord)
								.find((t) => str(t?.id) === taskId);
				if (
					seen !== undefined &&
					str(seen?.status) === 'processing' &&
					seenProcessing === undefined
				)
					seenProcessing = res?.doneAt;
				await sleep(100);
			}
		})();
		c.onCleanup('stop polling', async () => {
			polling = false;
			await poller;
		});

		const createdEvent = await stream.waitForEvent(
			(e) => e.type === 'task.created' && str(e.subject.task_id) === taskId,
			world.cfg.limitMs + 5000
		);
		check(createdEvent !== null, 'no task.created event within the limit');
		const createdMs = Math.max(0, createdEvent.t - t0);
		c.timing('task_created_ms', createdMs);
		check(
			createdMs <= world.cfg.limitMs,
			`task.created took ${Math.round(createdMs)} ms, over the limit`
		);

		const changed = await stream.waitForEvent(
			(e) =>
				e.type === 'task.status_changed' &&
				str(e.subject.task_id) === taskId &&
				str(e.subject.status) === 'processing',
			20_000
		);
		check(
			changed !== null,
			'no task.status_changed to processing within 20 s (is the worker running?)'
		);
		const changedEvent = asEvent(changed);
		checkEqual(str(changedEvent?.subject.previous_status), 'pending', 'previous status');
		check(
			stream.items.indexOf(createdEvent) < stream.items.indexOf(changed),
			'task.created arrived after task.status_changed'
		);
		const base = seenProcessing ?? t0;
		const changedMs = Math.max(0, changed.t - base);
		c.timing('task_status_changed_ms', changedMs);
		c.detail(
			'status_change_basis',
			seenProcessing === undefined
				? 'end of the task creation response (the poll did not observe processing)'
				: 'first poll that observed processing'
		);
		c.detail(
			'event_arrived_before_poll_saw_it',
			seenProcessing !== undefined && changed.t < seenProcessing
		);
		check(
			changedMs <= world.cfg.limitMs,
			`task.status_changed took ${Math.round(changedMs)} ms, over the limit`
		);
	});
});

test('A09 an event for P2 is not delivered', async ({ browser, world }) => {
	await runCase(world, browser, 'A09', async (c) => {
		const conn = await world.sharedConnection(c);
		// Notification types only: task events of an earlier case are not part of this question.
		const stream = await c.openStreamOrFail(conn, 'event stream', {
			types: 'notification.created,notification.read'
		});
		const p2Item = await world.createItem(c.rec, world.ids.p2Id, `qa-p4-a09-${randomHex(3)}`);
		await world.assignToA(c.rec, p2Item);
		const silence = world.cfg.silenceMs;
		const unexpected = await stream.waitFor((timed) => asEvent(timed) !== null, silence);
		check(unexpected === null, `an event arrived within ${silence} ms of assigning a P2 item`);
		// A control that the stream was alive: the same action in P1 does arrive.
		const p1Item = await world.createItem(c.rec, world.ids.p1Id, `qa-p4-a09-${randomHex(3)}`);
		const assignedAt = await world.assignToA(c.rec, p1Item);
		const arrived = await stream.waitForEvent(
			(e) => relatedIssue(e) === p1Item,
			world.cfg.limitMs + 5000
		);
		check(
			arrived !== null,
			'the control event for P1 never arrived, so the silence proves nothing'
		);
		c.timing('control_event_ms', Math.max(0, arrived.t - assignedAt));
		const events = stream.events();
		check(
			events.every((e) => e.projectId !== world.ids.p2Id && relatedIssue(e) !== p2Item),
			'an event concerning P2 was delivered'
		);
		checkEqual(events.length, 1, 'number of events on the stream');
	});
});

test('A10 resume with Last-Event-ID', async ({ browser, world }) => {
	await runCase(world, browser, 'A10', async (c) => {
		const conn = await world.sharedConnection(c);
		const types = 'notification.created';
		const first = await c.openStreamOrFail(conn, 'first stream', { types });
		const resume = first.resumeId();
		check(resume !== undefined, 'the first stream gave no position to resume from');
		await first.close();
		const items: string[] = [];
		for (const n of [1, 2]) {
			const id = await world.createItem(c.rec, world.ids.p1Id, `qa-p4-a10-${n}-${randomHex(3)}`);
			await world.assignToA(c.rec, id);
			items.push(id);
		}
		await sleep(3000); // let the worker move both into the feed, so they are backfilled
		const second = await c.openStreamOrFail(conn, 'resumed stream', { types, lastEventId: resume });
		check(
			!second.controls().some((control) => control.type === 'resync_required'),
			'the server asked for a resync instead of resuming'
		);
		const got: SvEvent[] = [];
		let from = 0;
		while (got.length < 2) {
			const next = await second.waitFor((timed) => asEvent(timed) !== null, 10_000, from);
			check(next !== null, `only ${got.length} of the 2 missed events arrived`);
			from = second.items.indexOf(next) + 1;
			const event = asEvent(next);
			if (event !== null) got.push(event);
		}
		const extra = await second.waitFor((timed) => asEvent(timed) !== null, 3000, from);
		check(extra === null, 'a third event arrived after the two missed ones');
		check(
			got[0] !== undefined &&
				got[1] !== undefined &&
				relatedIssue(got[0]) === items[0] &&
				relatedIssue(got[1]) === items[1],
			'the missed events are not the two assigned items in order'
		);
		check(got[0]?.frameId !== got[1]?.frameId, 'the two events share one id');
	});
});

// ---- A11 to A17: revocation ------------------------------------------------------------------

test('A11 revocation from Sylvode (personal settings)', async ({ browser, world }) => {
	await runCase(world, browser, 'A11', async (c) => {
		const conn = await world.sharedConnection(c);
		await conn.access(c.rec, 'mcp');
		const stream = await c.openStreamOrFail(conn, 'event stream');
		const context = await browser.newContext({
			locale: 'en-US',
			viewport: { width: 1280, height: 900 }
		});
		c.onCleanup('settings context', () => context.close());
		const page = await context.newPage();
		await signIn(page, world.cfg.uiOrigin, world.cfg.userA);
		let t0 = 0;
		let deleteStatus = 0;
		await personalSettings(page, world.cfg.uiOrigin, {
			disconnectGrantId: conn.grantId,
			onDeleteResponse: (status) => {
				deleteStatus = status;
				t0 = mono();
			}
		});
		check(
			deleteStatus >= 200 && deleteStatus < 300,
			`the disconnect request answered HTTP ${deleteStatus}`
		);
		c.detail('trigger', 'DELETE answer as seen by the browser (Playwright response event)');
		const revokedAt = await expectRevoked(c, stream, t0, 'user_revoked', 'revoked_ms');
		world.forgetShared();

		const another = await world.createItem(c.rec, world.ids.p1Id, `qa-p4-a11-${randomHex(3)}`);
		await world.assignToA(c.rec, another);
		await sleep(3000);
		check(
			!stream.items.slice(revokedAt + 1).some((timed) => asEvent(timed) !== null),
			'an event was delivered after revoked'
		);
		await expectDead(c, conn);
	});
});

test('A12 revocation from the client', async ({ browser, world }) => {
	await runCase(world, browser, 'A12', async (c) => {
		const { conn, stream } = await connectAndStream(c);
		const res = await conn.revokeFromClient(c.rec);
		checkEqual(res.status, 200, 'revoke status');
		check(res.text.trim() === '', 'the revoke answer has a body');
		await expectRevoked(c, stream, res.doneAt, 'client_revoked', 'revoked_ms');
		await expectDead(c, conn);
	});
});

test('A13 revocation by the workspace admin', async ({ browser, world }) => {
	await runCase(world, browser, 'A13', async (c) => {
		const { conn, grantId, stream } = await connectAndStream(c);
		const context = await browser.newContext({
			locale: 'en-US',
			viewport: { width: 1280, height: 900 }
		});
		c.onCleanup('admin context', () => context.close());
		const page = await context.newPage();
		await signIn(page, world.cfg.uiOrigin, world.cfg.userB);
		let t0 = 0;
		let deleteStatus = 0;
		await workspaceDisconnect(
			page,
			world.cfg.uiOrigin,
			world.ids.workspaceId,
			grantId,
			(status) => {
				deleteStatus = status;
				t0 = mono();
			}
		);
		check(
			deleteStatus >= 200 && deleteStatus < 300,
			`the workspace disconnect answered HTTP ${deleteStatus}`
		);
		await expectRevoked(c, stream, t0, 'workspace_admin_revoked', 'revoked_ms');
		await expectDead(c, conn);
	});
});

test('A14 password change', async ({ browser, world }) => {
	await runCase(world, browser, 'A14', async (c) => {
		const { conn, stream } = await connectAndStream(c);
		const changed = `Chg-${randomHex(8)}-1!`;
		redactor.register(changed, 'password:changed');
		c.onCleanup('restore the password of user A', () => world.restoreUserA(new Recorder()));
		const res = await world.userA.request(
			c.rec,
			'A changes the password',
			'PUT',
			'/api/v1/auth/me/password',
			{
				current_password: world.cfg.userA.password,
				new_password: changed
			}
		);
		check(
			res.status === 200 && asRecord(res.json)?.code === 0,
			`the password change was refused (HTTP ${res.status})`
		);
		await expectRevoked(c, stream, res.doneAt, 'password_changed', 'revoked_ms');
		await expectDead(c, conn);
	});
});

test('A15 account disabled by the instance admin', async ({ browser, world }) => {
	await runCase(world, browser, 'A15', async (c) => {
		const { conn, stream } = await connectAndStream(c);
		c.onCleanup('re-enable user A', () => world.restoreUserA(new Recorder()));
		const res = await world.admin.request(
			c.rec,
			'the administrator disables A',
			'PATCH',
			`/api/v1/admin/users/${world.ids.userAId}/status`,
			{ is_active: false }
		);
		check(
			res.status === 200 && asRecord(res.json)?.code === 0,
			`disabling the account was refused (HTTP ${res.status})`
		);
		await expectRevoked(c, stream, res.doneAt, 'account_disabled', 'revoked_ms');
		await expectDead(c, conn);
	});
});

test('A16 a refresh token used twice', async ({ browser, world }) => {
	await runCase(world, browser, 'A16', async (c) => {
		const { conn, stream } = await connectAndStream(c);
		const env = world.env();
		const spent = conn.refreshToken;
		const first = await conn.rotate(c.rec, 'mcp', 'token: first use');
		checkEqual(first.res.status, 200, 'first refresh');
		const second = await refreshGrant(env, c.rec, {
			refreshToken: spent,
			resource: env.disc.mcpResource,
			label: 'token: second use of the same refresh token'
		});
		checkEqual(second.res.status, 400, 'second refresh status');
		checkEqual(second.error, 'invalid_grant', 'second refresh error');
		await expectRevoked(c, stream, second.res.doneAt, 'refresh_reuse', 'revoked_ms');
		await expectDead(c, conn);
	});
});

test('A17 an authorization code redeemed twice', async ({ browser, world }) => {
	await runCase(world, browser, 'A17', async (c) => {
		const connected = await world.connect(c, { projects: projectScope(world) });
		const stream = await c.openStreamOrFail(connected.conn, 'event stream');
		const env = world.env();
		const replay = await exchangeCode(env, c.rec, {
			...connected.authorization,
			resource: env.disc.mcpResource,
			label: 'token: the same code again'
		});
		checkEqual(replay.res.status, 400, 'replayed code status');
		checkEqual(replay.error, 'invalid_grant', 'replayed code error');
		await expectRevoked(c, stream, replay.res.doneAt, 'code_replay', 'revoked_ms');
		await expectDead(c, connected.conn);
	});
});

// ---- A18 to A19: refusals at the consent page ---------------------------------------------------

test('A18 deny, and "I do not have an account"', async ({ browser, world }) => {
	await runCase(world, browser, 'A18', async (c) => {
		const countGrants = async (): Promise<number> =>
			itemsOf(
				await world.userA.data(
					c.rec,
					'A lists connected applications',
					'GET',
					'/api/v1/me/connected-apps'
				)
			).length;
		const countUsers = async (): Promise<number> => {
			const total = dig(
				await world.admin.data(c.rec, 'count users', 'GET', '/api/v1/admin/users?limit=1'),
				'total'
			);
			check(typeof total === 'number', 'the user list gives no total to compare');
			return total;
		};
		const grantsBefore = await countGrants();
		const usersBefore = await countUsers();

		for (const [mode, description] of [
			['deny', 'consent_denied'],
			['no_account', 'account_required']
		] as const) {
			const started = await world.authorize(
				c,
				mode === 'deny' ? { mode, login: world.cfg.userA } : { mode }
			);
			const target = await started.listener.waitForCallback(20_000);
			const verdict = parseCallback(target, {
				path: CALLBACK_PATH,
				iss: world.cfg.uiOrigin,
				state: started.state
			});
			check(
				!verdict.ok && verdict.kind === 'error',
				`${mode}: the redirect was not a verified error (${JSON.stringify(redactor.json(verdict))})`
			);
			checkEqual(verdict.error, 'access_denied', `${mode} error`);
			checkEqual(verdict.description, description, `${mode} description`);
			check(
				!new URL(target, 'http://127.0.0.1').searchParams.has('code'),
				`${mode}: the redirect carries a code`
			);
		}
		checkEqual(await countGrants(), grantsBefore, 'connected applications of A after the refusals');
		checkEqual(await countUsers(), usersBefore, 'number of users after the refusals');
	});
});

test('A19 a localhost redirect URI', async ({ browser, world }) => {
	await runCase(world, browser, 'A19', async (c) => {
		const started = await world.authorize(c, { mode: 'deny' }, { redirectHost: 'localhost' });
		checkEqual(started.view, 'invalid', 'the page the request ended on');
		check(
			await started.listener.stayedQuiet(3000),
			'the browser was redirected to the localhost address'
		);
		checkEqual(started.listener.strays.length, 0, 'stray requests at the listener');
	});
});

// ---- A20: the existing regression scripts -----------------------------------------------------------

test('A20 existing regression scripts', async ({ browser, world }) => {
	test.setTimeout(700_000);
	await runCase(world, browser, 'A20', async (c) => {
		const cfg = world.cfg;
		check(repoRootOf(cfg) !== '', 'no repository root');
		const missing: string[] = [];
		const failed: string[] = [];
		const runs: Array<{
			name: string;
			exitCode: number | null;
			durationMs: number;
			mode: string;
			outputTail: string;
		}> = [];
		for (const spec of specsFor(cfg)) {
			if (!scriptExists(spec, cfg)) {
				missing.push(`${spec.name} does not exist at ${spec.script}`);
				continue;
			}
			if (spec.kind === 'mcp' && cfg.oprBotToken === undefined && cfg.a20Override === undefined) {
				failed.push(
					`${spec.name} needs OAUTH_E2E_OPR_BOT_TOKEN (a workspace bot token of the workspace the MCP server is bound to)`
				);
				continue;
			}
			const run = await runSpec(cfg, spec);
			runs.push({
				name: run.name,
				exitCode: run.exitCode,
				durationMs: run.durationMs,
				mode: run.mode,
				outputTail: run.outputTail
			});
			c.rec.note(`${run.name}: exit ${run.exitCode}; output tail:\n${run.outputTail}`);
			if (run.exitCode !== 0)
				failed.push(
					run.exitCode === null
						? `${run.name} could not be run to completion (not started, killed or timed out)`
						: `${run.name} exited with ${run.exitCode}`
				);
		}
		c.detail(
			'runs',
			runs.map(({ outputTail: _ignored, ...rest }) => rest)
		);
		check(missing.length === 0, `regression script missing: ${missing.join('; ')}`);
		check(failed.length === 0, failed.join('; '));
	});
});
