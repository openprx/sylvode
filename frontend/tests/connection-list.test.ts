/**
 * The Execution section with several connections per project: the plural routes, the list read
 * at the boundary, add / per-card edit / remove on the list, the tmux option that is off for a
 * CLI that already has a tmux connection (and the 409 that says the same), the permanent
 * permission warning, and the session rows of the status.
 *
 * Run standalone: `bun tests/connection-list.test.ts`
 */
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { apiClient } from '../src/lib/api/client';
import { botConnectionApi } from '../src/lib/api/bot-connection';
import { anyWaiting, dropItem, putItem } from '../src/lib/project/connection/list';
import { routeServerMessage, tmuxTakenBy } from '../src/lib/project/connection/server-errors';
import {
	runsWithoutPermissionChecks,
	sessionChip,
	statusBadges
} from '../src/lib/project/connection/status';
import {
	attachCommand,
	blockReason,
	channelErrorText,
	tmuxBlock
} from '../src/lib/project/connection/tmux';
import {
	parseList,
	parseRead,
	parseStatus,
	parseConnection,
	type Connection,
	type ConnectionItem
} from '../src/lib/project/connection/wire';
import { defaultDraft } from '../src/lib/project/connection/advanced';

await installSvelteSsr();
const { default: ConnectedView } =
	await import('../src/lib/project/connection/ConnectedView.svelte');
const { default: AdvancedFields } =
	await import('../src/lib/project/connection/AdvancedFields.svelte');
const { default: StatusRows } = await import('../src/lib/project/connection/StatusRows.svelte');

const suite = new Suite('connection-list');

function connectionJson(
	bot: string,
	name: string,
	channel: 'tmux' | 'headless',
	cli = 'claude',
	version = 1
) {
	return {
		project_id: 'p1',
		version,
		enabled: true,
		bot: { id: bot, name, token_prefix: 'opr_1a2b', token_active: true },
		runner: {
			url: 'https://runner.example.net:9090',
			webhook_id: `h-${bot}`,
			credentials_issued_at: '2026-11-21T10:00:00Z',
			shared_with_projects: [{ id: 'p1', name: 'Core' }]
		},
		execution: {
			channel,
			cli,
			working_directory: '/srv/dev/work',
			...(channel === 'tmux' ? { session_name: `QLA-${cli}` } : {}),
			on_complete_state: 'auto',
			on_fail_label_id: null,
			approval_gate: 'none'
		},
		governance: {
			max_domain_level: 'voter',
			can_veto_human_consensus: false,
			reason_min_length: 50,
			domain_overrides: {}
		}
	};
}

const statusJson = (extra: Record<string, unknown> = {}) => ({
	runner: { state: 'online', reported_at: '2026-11-22T10:00:00Z', version: '0.9.0', warning: null },
	cli: { state: 'ready', version: '2.1.0' },
	tmux: { state: 'ready', version: '3.5' },
	last_delivery: { state: 'none', at: null, project_key: null, task_id: null, reason_code: null },
	...extra
});

function conn(json: ReturnType<typeof connectionJson>): Connection {
	const parsed = parseConnection(json);
	assert(parsed !== null, `parsed ${json.bot.name}`);
	return parsed;
}

const tmuxCard = conn(connectionJson('b1', 'Build bot', 'tmux'));
const headlessCard = conn(connectionJson('b2', 'Review bot', 'headless', 'codex'));

/** Records the requests the API client makes and answers each with `answer`. */
async function withFetch<T>(
	answer: (
		url: string,
		init: RequestInit | undefined
	) => { code: number; message?: string; data?: unknown },
	body: (calls: Array<{ url: string; method: string; body: unknown }>) => Promise<T>
): Promise<T> {
	const original = globalThis.fetch;
	const calls: Array<{ url: string; method: string; body: unknown }> = [];
	globalThis.fetch = async (input, init) => {
		const url = String(input);
		calls.push({
			url,
			method: init?.method ?? 'GET',
			body: typeof init?.body === 'string' ? JSON.parse(init.body) : null
		});
		const reply = answer(url, init);
		return new Response(
			JSON.stringify({
				code: reply.code,
				message: reply.message ?? 'ok',
				data: reply.data ?? null
			}),
			{ headers: { 'Content-Type': 'application/json' } }
		);
	};
	apiClient.setToken('synthetic-access-token');
	try {
		return await body(calls);
	} finally {
		globalThis.fetch = original;
		apiClient.clearAuth();
	}
}

// --- the plural routes ------------------------------------------------------------------------------

await suite.checkAsync('every call goes to the plural routes, with the bot id', async () => {
	const write = { connection: connectionJson('b 1', 'Build bot', 'tmux'), status: statusJson() };
	await withFetch(
		() => ({ code: 0, data: write }),
		async (calls) => {
			await botConnectionApi.connect('p1', {
				bot: { name: 'Build bot' },
				runner_url: 'https://r',
				cli: 'claude',
				working_directory: '/a'
			});
			await botConnectionApi.update('p1', 'b 1', { expected_version: 2, working_directory: '/b' });
			await botConnectionApi.disable('p1', 'b 1', 2);
			await botConnectionApi.enable('p1', 'b 1', 3);
			await botConnectionApi.rotate('p1', 'b 1', 4);
			await botConnectionApi.get('p1', 'b 1');
			await botConnectionApi.remove('p1', 'b 1', 5);
			const seen = calls.map((call) => `${call.method} ${call.url}`);
			assertEqual(
				seen.join('\n'),
				[
					'POST /api/v1/projects/p1/bot-connections',
					'PATCH /api/v1/projects/p1/bot-connections/b%201',
					'POST /api/v1/projects/p1/bot-connections/b%201/disable',
					'POST /api/v1/projects/p1/bot-connections/b%201/enable',
					'POST /api/v1/projects/p1/bot-connections/b%201/rotate',
					'GET /api/v1/projects/p1/bot-connections/b%201',
					'DELETE /api/v1/projects/p1/bot-connections/b%201?expected_version=5'
				].join('\n'),
				'requests'
			);
			assertEqual(calls[1]?.body instanceof Object, true, 'update has a body');
			assertEqual(
				(calls[1]?.body as Record<string, unknown>).expected_version,
				2,
				'update carries the version'
			);
			assertEqual(
				(calls[3]?.body as Record<string, unknown>).expected_version,
				3,
				'enable version'
			);
		}
	);
	await withFetch(
		() => ({ code: 0, data: { connections: [] } }),
		async (calls) => {
			const result = await botConnectionApi.list('p1');
			assertEqual(calls[0]?.url, '/api/v1/projects/p1/bot-connections', 'list url');
			assert(result.ok && result.value.kind === 'full' && result.value.items.length === 0, 'empty');
		}
	);
});

// --- the list read at the boundary ----------------------------------------------------------------------

suite.check(
	'the list: cards with their status, reduced member views, nothing half-understood',
	() => {
		const list = parseList({
			connections: [
				{ connection: connectionJson('b1', 'Build bot', 'tmux'), status: statusJson() },
				{ connection: connectionJson('b2', 'Review bot', 'headless', 'codex'), status: null }
			]
		});
		assert(list?.kind === 'full', 'full');
		assertEqual(list.items.length, 2, 'two cards');
		assertEqual(
			list.items[0]?.connection.execution.session_name,
			'QLA-claude',
			'tmux session name'
		);
		assertEqual(list.items[1]?.connection.execution.session_name, null, 'headless has none');
		assertEqual(list.items[1]?.status, null, 'no status is no status');
		assertEqual(parseList({ connections: [] })?.kind, 'full', 'an empty list is a list');
		const reduced = parseList({
			connections: [{ bot: { id: 'b1', name: 'Build bot' }, enabled: true, cli: 'claude' }]
		});
		assertEqual(reduced?.kind, 'reduced', 'members get the reduced views');
		assertEqual(
			parseList({ connections: [{ connection: { version: 'x' } }] }),
			null,
			'broken item'
		);
		assertEqual(parseList({ connection: connectionJson('b1', 'x', 'tmux') }), null, 'not a list');
		assertEqual(parseList(null), null, 'null');
		assertEqual(parseRead({ connection: null, status: null }), null, 'a single read needs one');
	}
);

suite.check('the session rows of the status are tolerant and absent when not sent', () => {
	const bare = parseStatus(statusJson());
	assertEqual(bare?.tmux_session, null, 'no session row');
	assertEqual(bare?.bypass, null, 'bypass unknown');
	const full = parseStatus(
		statusJson({
			bypass: true,
			tmux_session: {
				session_name: 'QLA-claude',
				status: 'warn',
				state: 'running',
				created_by: 'operator',
				hooks: 'none',
				permissions: 'bypass',
				mcp: 'stale'
			}
		})
	);
	assertEqual(full?.bypass, true, 'bypass');
	assertEqual(full?.tmux_session?.created_by, 'operator', 'created by');
	assertEqual(full?.tmux_session?.mcp, 'stale', 'mcp');
	const odd = parseStatus(
		statusJson({ bypass: 'yes', tmux_session: { state: 'asleep', hooks: 7, status: 'warn' } })
	);
	assert(odd !== null, 'an odd session row does not fail the status');
	assertEqual(odd.bypass, null, 'a non-boolean flag is unknown');
	assertEqual(odd.tmux_session?.state, null, 'a state outside the vocabulary is dropped');
	assertEqual(odd.tmux_session?.hooks, null, 'a hooks value of another type is dropped');
	assertEqual(parseStatus(statusJson({ tmux_session: 'x' }))?.tmux_session, null, 'not a row');
});

suite.check('session row: who started it, the badges, the permanent warning rule', () => {
	const row = (state: string, created_by?: string, extra: Record<string, unknown> = {}) =>
		parseStatus(statusJson({ tmux_session: { status: 'ok', state, created_by, ...extra } }));
	const key = (status: ReturnType<typeof parseStatus>) =>
		sessionChip(status?.tmux_session ?? null)?.key;
	const base = 'execution.connection.status.session.';
	assertEqual(key(row('running', 'runner')), `${base}runningRunner`, 'started by runner');
	assertEqual(key(row('running', 'operator')), `${base}runningOperator`, 'started by you');
	assertEqual(key(row('running')), `${base}running`, 'origin unknown');
	assertEqual(key(row('absent')), `${base}absent`, 'not started');
	assertEqual(key(row('cli_not_running')), `${base}cliNotRunning`, 'cli not running');
	assertEqual(key(row('ambiguous')), `${base}ambiguous`, 'ambiguous');
	assertEqual(key(row('dialog')), `${base}dialog`, 'dialog');
	assertEqual(sessionChip(null), null, 'no row, no chip');
	assertEqual(statusBadges(parseStatus(statusJson())).length, 0, 'nothing to flag');
	assertEqual(statusBadges(null).length, 0, 'no status');
	const badges = statusBadges(
		row('running', 'runner', { permissions: 'bypass', hooks: 'none', mcp: 'missing' })
	).map((badge) => badge.key.replace('execution.connection.status.badge.', ''));
	assertEqual(badges.join(','), 'bypass,noHooks,noTools', 'the three badges');
	assertEqual(
		statusBadges(row('running', 'runner', { mcp: 'stale' }))
			.map((b) => b.key)
			.join(),
		'execution.connection.status.badge.staleTools',
		'stale tools'
	);
	// The warning stays unless a report says the checks are on.
	assertEqual(runsWithoutPermissionChecks(null), true, 'no status: the default applies');
	assertEqual(runsWithoutPermissionChecks(parseStatus(statusJson())), true, 'no report on it');
	assertEqual(
		runsWithoutPermissionChecks(parseStatus(statusJson({ bypass: true }))),
		true,
		'bypass'
	);
	assertEqual(
		runsWithoutPermissionChecks(parseStatus(statusJson({ bypass: false }))),
		false,
		'off'
	);
	assertEqual(
		runsWithoutPermissionChecks(row('running', 'runner', { permissions: 'normal' })),
		false,
		'normal'
	);
	assertEqual(
		runsWithoutPermissionChecks(row('running', 'runner', { permissions: 'bypass' })),
		true,
		'session bypass'
	);
});

// --- add, per-card edit, remove --------------------------------------------------------------------------

suite.check(
	'add puts the new card in bot-name order, edit replaces only its own card, remove drops it',
	() => {
		const item = (
			bot: string,
			name: string,
			channel: 'tmux' | 'headless',
			version = 1
		): ConnectionItem => ({
			connection: conn(connectionJson(bot, name, channel, 'claude', version)),
			status: null
		});
		let items: ConnectionItem[] = [];
		items = putItem(items, item('b2', 'Review bot', 'headless'));
		items = putItem(items, item('b1', 'Build bot', 'tmux'));
		assertEqual(
			items.map((i) => i.connection.bot.name).join(),
			'Build bot,Review bot',
			'added, sorted'
		);
		assertEqual(items.length, 2, 'two cards');
		const edited = putItem(items, item('b2', 'Review bot', 'headless', 7));
		assertEqual(edited.length, 2, 'an edit adds no card');
		assertEqual(edited[1]?.connection.version, 7, 'the edited card is new');
		assertEqual(edited[0]?.connection.version, 1, 'the other card is untouched');
		assertEqual(items[1]?.connection.version, 1, 'the input list is not changed');
		const left = dropItem(edited, 'b1');
		assertEqual(left.map((i) => i.connection.bot.id).join(), 'b2', 'removed');
		assertEqual(dropItem(left, 'nope').length, 1, 'removing an unknown bot changes nothing');
		assertEqual(dropItem(left, 'b2').length, 0, 'an empty list is the empty state');
		assertEqual(anyWaiting(items), false, 'nobody waits');
		const waiting = parseStatus(statusJson({ runner: { state: 'waiting' } }));
		assertEqual(anyWaiting([{ ...items[0]!, status: waiting }, items[1]!]), true, 'one waits');
	}
);

// --- the tmux option --------------------------------------------------------------------------------------

suite.check(
	'tmux is off for a CLI that already has a tmux connection, and for any CLI but claude',
	() => {
		const connections = [tmuxCard, headlessCard];
		const taken = tmuxBlock(connections, 'claude', null);
		assertEqual(taken?.kind, 'taken', 'claude is taken');
		assert(taken?.kind === 'taken' && taken.bot === 'Build bot', 'names the holder');
		assert(taken?.kind === 'taken' && taken.session === 'QLA-claude', 'names the session');
		assertEqual(tmuxBlock(connections, 'claude', 'b1'), null, 'the holder may keep its own tmux');
		assertEqual(tmuxBlock([headlessCard], 'claude', null), null, 'headless holds no session');
		assertEqual(tmuxBlock([], 'claude', null), null, 'an empty project');
		assertEqual(tmuxBlock([], 'codex', null)?.kind, 'cli', 'only claude can use tmux');
		assertEqual(tmuxBlock([], 'opencode', null)?.kind, 'cli', 'opencode neither');
		const text = (key: string, values?: Record<string, string>) =>
			`${key}|${JSON.stringify(values ?? {})}`;
		assertEqual(
			blockReason({ kind: 'taken', bot: 'Build bot', session: 'QLA-claude' }, text),
			'execution.connection.advanced.channel.taken|{"bot":"Build bot","session":"QLA-claude"}',
			'"<bot> already uses tmux session <KEY>-<cli>"'
		);
		assertEqual(attachCommand('QLA-claude'), 'tmux attach -t QLA-claude', 'attach command');
	}
);

suite.check(
	'the 409 of a second tmux connection is a channel field error with the same reason',
	() => {
		const message = 'channel: project already has a tmux connection for claude (bot Build bot)';
		assertEqual(routeServerMessage(message).field, 'channel', 'field');
		assertEqual(
			routeServerMessage(message).text,
			'project already has a tmux connection for claude (bot Build bot)',
			'text'
		);
		assertEqual(tmuxTakenBy(message)?.bot, 'Build bot', 'the holder');
		assertEqual(tmuxTakenBy(message)?.cli, 'claude', 'the CLI');
		assertEqual(tmuxTakenBy('channel: something else'), null, 'another channel refusal');
		assertEqual(tmuxTakenBy('project already has a connection for this bot'), null, 'another 409');
		assertEqual(
			routeServerMessage('cli: only claude can use the tmux channel').field,
			'channel',
			'cli rule'
		);
		const text = (key: string, values?: Record<string, string>) =>
			`${key}|${JSON.stringify(values ?? {})}`;
		assertEqual(
			channelErrorText(message, [tmuxCard], text),
			'execution.connection.advanced.channel.taken|{"bot":"Build bot","session":"QLA-claude"}',
			'same text as the disabled option'
		);
		assertEqual(
			channelErrorText(message, [], text),
			'execution.connection.advanced.channel.takenNoSession|{"bot":"Build bot"}',
			'holder not in the list (changed meanwhile)'
		);
		assertEqual(
			channelErrorText('channel: cannot be tmux today', [], text),
			'cannot be tmux today',
			'other refusals show the server words'
		);
	}
);

await suite.checkAsync(
	'a refused connect carries the 409 message and the card list is untouched',
	async () => {
		const refusal = 'channel: project already has a tmux connection for claude (bot Build bot)';
		await withFetch(
			() => ({ code: 409, message: refusal }),
			async () => {
				const result = await botConnectionApi.connect('p1', {
					bot: { name: 'Second' },
					runner_url: 'https://r',
					cli: 'claude',
					working_directory: '/a',
					channel: 'tmux'
				});
				assert(!result.ok, 'refused');
				assertEqual(result.code, 409, 'code');
				assertEqual(result.message, refusal, 'message');
				assertEqual(result.current, null, 'a business 409 has no current connection');
			}
		);
	}
);

// --- what renders --------------------------------------------------------------------------------------------

const viewProps = (connection: Connection, status: ReturnType<typeof parseStatus>) => ({
	workspaceId: 'w1',
	projectName: 'Core',
	connection,
	connections: [tmuxCard, headlessCard],
	status,
	now: new Date('2026-11-22T10:05:00Z'),
	refreshing: false,
	onrefresh: () => {},
	onchanged: () => {},
	onremoved: () => {},
	onconflict: () => {}
});

await suite.checkAsync(
	'a tmux card: session name, attach command, the permission warning, actions',
	async () => {
		const html = await renderToHtml(ConnectedView, viewProps(tmuxCard, parseStatus(statusJson())));
		assert(html.includes('Build bot'), 'bot name');
		assert(/tmux-session-name[^>]*>QLA-claude</.test(html), 'session name');
		assert(html.includes('tmux attach -t QLA-claude'), 'attach command');
		assert(html.includes('execution.connection.workingDirectoryTmux'), 'tmux directory label');
		assert(html.includes('execution.connection.cli.tmuxOnlyClaude'), 'cli is fixed to claude');
		assert(html.includes('execution.connection.tmux.behaviour'), 'static text of U2');
		assert(html.includes('data-testid="bypass-warning"'), 'the warning with no report on it');
		assert(html.includes('/srv/dev/work'), 'working directory');
		for (const action of ['disable', 'rotate', 'remove'])
			assert(html.includes(`execution.connection.actions.${action}`), action);
		assert(html.includes('execution.connection.state.enabled'), 'enabled state');
		// The CLI row has no "Change" while the connection is tmux: one for the runner address and
		// one for the directory remain.
		assertEqual(html.split('execution.connection.change').length - 1, 2, 'no change for the CLI');
	}
);

await suite.checkAsync(
	'a headless card has no session lines and drops the warning when checks are on',
	async () => {
		const normal = parseStatus(statusJson({ tmux: null, bypass: false }));
		const html = await renderToHtml(ConnectedView, viewProps(headlessCard, normal));
		assertEqual(html.includes('data-testid="tmux-session"'), false, 'no session block');
		assertEqual(html.includes('tmux attach'), false, 'no attach command');
		assertEqual(html.includes('data-testid="bypass-warning"'), false, 'no warning: checks are on');
		assertEqual(html.split('execution.connection.change').length - 1, 3, 'the CLI is editable');
		const warned = await renderToHtml(
			ConnectedView,
			viewProps(headlessCard, parseStatus(statusJson({ tmux: null, bypass: true })))
		);
		assert(warned.includes('data-testid="bypass-warning"'), 'warning when the report says bypass');
	}
);

await suite.checkAsync('two cards of one project render independently', async () => {
	const first = await renderToHtml(ConnectedView, viewProps(tmuxCard, null));
	const second = await renderToHtml(ConnectedView, viewProps(headlessCard, null));
	assert(first.includes('data-bot-id="b1"') && !first.includes('Review bot'), 'first card');
	assert(second.includes('data-bot-id="b2"') && !second.includes('Build bot'), 'second card');
});

await suite.checkAsync(
	'the status rows show the session, who started it, and the badges',
	async () => {
		const status = parseStatus(
			statusJson({
				tmux_session: {
					status: 'warn',
					state: 'running',
					created_by: 'operator',
					hooks: 'none',
					permissions: 'bypass',
					mcp: 'missing'
				}
			})
		);
		const props = {
			status,
			cli: 'claude',
			showTmux: true,
			now: new Date('2026-11-22T10:05:00Z'),
			refreshing: false,
			onrefresh: () => {}
		};
		const html = await renderToHtml(StatusRows, props);
		assert(html.includes('execution.connection.status.session.runningOperator'), 'started by you');
		for (const badge of ['bypass', 'noHooks', 'noTools'])
			assert(html.includes(`execution.connection.status.badge.${badge}`), badge);
		assert(
			html.includes('execution.connection.status.badge.noHooksHint'),
			'screen-only turn tracking'
		);
		const bare = await renderToHtml(StatusRows, { ...props, status: parseStatus(statusJson()) });
		assertEqual(
			bare.includes('status.session.'),
			false,
			'absent when the status does not carry it'
		);
		assertEqual(bare.includes('status-badges'), false, 'no badges');
		const headless = await renderToHtml(StatusRows, { ...props, showTmux: false });
		assertEqual(headless.includes('execution.connection.status.session.label'), false, 'headless');
	}
);

const advancedProps = (extra: Record<string, unknown>) => ({
	draft: { ...defaultDraft(), channel: 'headless' as const },
	errors: {
		level: null,
		reasonLength: null,
		veto: null,
		domains: [],
		state: null
	},
	states: [],
	labels: [],
	domains: [],
	...extra
});

await suite.checkAsync(
	'the form: tmux option disabled with the reason, enabled otherwise',
	async () => {
		const taken = await renderToHtml(
			AdvancedFields,
			advancedProps({ tmuxBlock: { kind: 'taken', bot: 'Build bot', session: 'QLA-claude' } })
		);
		assert(taken.includes('data-testid="tmux-disabled-reason"'), 'the reason is shown');
		assert(taken.includes('execution.connection.advanced.channel.taken'), 'the reason text');
		const tmuxButton = /<button[^>]*data-value="tmux"[^>]*>/.exec(taken);
		assert(
			tmuxButton !== null && tmuxButton[0].includes('aria-disabled="true"'),
			'the tmux option is disabled'
		);
		const free = await renderToHtml(AdvancedFields, advancedProps({ tmuxBlock: null }));
		assertEqual(free.includes('tmux-disabled-reason'), false, 'no reason when free');
		const freeButton = /<button[^>]*data-value="tmux"[^>]*>/.exec(free);
		assert(
			freeButton !== null && !freeButton[0].includes('aria-disabled'),
			'the tmux option is enabled'
		);
		const cli = await renderToHtml(AdvancedFields, advancedProps({ tmuxBlock: { kind: 'cli' } }));
		assert(cli.includes('execution.connection.advanced.channel.onlyClaude'), 'only claude');
		const refused = await renderToHtml(
			AdvancedFields,
			advancedProps({ channelError: 'Build bot already uses tmux session QLA-claude' })
		);
		assert(refused.includes('data-testid="channel-error"'), 'the 409 shows as a field error');
		assert(refused.includes('Build bot already uses tmux session QLA-claude'), 'with its text');
		// A saved tmux connection reads as tmux even when its CLI is held: it is the holder.
		const own = await renderToHtml(
			AdvancedFields,
			advancedProps({
				draft: { ...defaultDraft(), channel: 'tmux' as const },
				tmuxBlock: { kind: 'taken', bot: 'Build bot', session: 'QLA-claude' }
			})
		);
		assertEqual(own.includes('tmux-disabled-reason'), false, 'no reason while tmux is the choice');
	}
);

// --- the old paths are gone --------------------------------------------------------------------------------------

function sources(dir: string): Array<{ file: string; text: string }> {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return sources(path);
		return /\.(ts|svelte)$/.test(entry.name)
			? [{ file: path, text: readFileSync(path, 'utf8') }]
			: [];
	});
}

suite.check(
	'no call to the singular route or to execution-settings is left in the frontend',
	() => {
		const files = sources('src');
		assert(files.length > 100, 'the sources were found');
		for (const { file, text } of files) {
			assert(!/execution-settings/.test(text), `${file} mentions execution-settings`);
			assert(
				!/bot-connection(?![s\-.])/.test(text.replace(/#lib\/api\/bot-connection/g, '')),
				`${file} mentions the singular route`
			);
		}
	}
);

finish(suite.result());
