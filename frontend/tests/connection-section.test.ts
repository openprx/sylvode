/**
 * The Execution section (the bot connection): client validation that mirrors the server, the
 * status chip of every state, the answers read at the boundary, the advanced draft, and the
 * rules of the one-time configuration block.
 *
 * Run standalone: `bun tests/connection-section.test.ts`
 */
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import {
	NO_BLOCK,
	blockFile,
	canFinish,
	discard,
	finish as finishBlock,
	needsLeaveConfirmation,
	setStored,
	showBlock,
	textareaRows,
	visibleText
} from '../src/lib/project/connection/block-state';
import {
	changedFields,
	connectFields,
	defaultDraft,
	draftOf,
	onCompleteValue,
	validateDraft
} from '../src/lib/project/connection/advanced';
import { routeServerMessage } from '../src/lib/project/connection/server-errors';
import {
	componentChip,
	deliveryChip,
	deliveryValue,
	relativeTime,
	runnerChip,
	shouldPollWaiting
} from '../src/lib/project/connection/status';
import {
	ERRORS,
	normalizeDirectory,
	normalizeRunnerUrl,
	parseReasonLength,
	validateAdvanced,
	validateBotChoice,
	validateBotName,
	validateRunnerUrl,
	validateWorkingDirectory
} from '../src/lib/project/connection/validate';
import {
	parseBlock,
	parseConnection,
	parseRead,
	parseRemove,
	parseStatus,
	parseWrite
} from '../src/lib/project/connection/wire';

await installSvelteSsr();
const { default: ConfigBlock } = await import('../src/lib/project/connection/ConfigBlock.svelte');

const suite = new Suite('connection-section');

const connectionJson = {
	project_id: 'p1',
	version: 3,
	enabled: true,
	bot: { id: 'b1', name: 'Build bot', token_prefix: 'opr_1a2b', token_active: true },
	runner: {
		url: 'https://runner.example.net:9090',
		webhook_id: 'h1',
		credentials_issued_at: '2026-11-21T10:00:00Z',
		shared_with_projects: [{ id: 'p1', name: 'Core' }]
	},
	execution: {
		channel: 'tmux',
		cli: 'claude',
		working_directory: '/home/dev/work',
		on_complete_state: 'state:done',
		on_fail_label_id: null,
		approval_gate: 'none'
	},
	governance: {
		max_domain_level: 'voter',
		can_veto_human_consensus: false,
		reason_min_length: 50,
		domain_overrides: { code_review: 'advisor' }
	}
};
const SECRET = 'opr_' + 'ab'.repeat(32);
const BLOCK_TEXT = `[[ai_task_callback.bots]]\ntoken = "${SECRET}"\n`;

// --- validation mirrors the server ------------------------------------------------------------

suite.check('bot name: required, at most 100 characters', () => {
	assertEqual(validateBotName(''), ERRORS.nameRequired, 'empty');
	assertEqual(validateBotName('   '), ERRORS.nameRequired, 'blank');
	assertEqual(validateBotName('x'.repeat(100)), null, '100');
	assertEqual(validateBotName('x'.repeat(101)), ERRORS.nameLength, '101');
	assertEqual(validateBotName('好'.repeat(100)), null, 'characters, not bytes');
	assertEqual(validateBotChoice(null), ERRORS.botChoose, 'no bot chosen');
	assertEqual(validateBotChoice('b1'), null, 'a bot');
});

suite.check('runner address: http(s), no credentials, query or fragment; one slash dropped', () => {
	for (const ok of [
		'https://r.example.net:9090',
		'http://127.0.0.1:9',
		'https://r/p/',
		' https://r '
	])
		assertEqual(validateRunnerUrl(ok), null, ok);
	for (const bad of [
		'',
		'r.example.net',
		'ftp://r',
		'HTTP://r',
		'https://',
		'https://u:p@r',
		'https://r?x=1',
		'https://r#f',
		'https://r//',
		'https://r a',
		'https://' + 'a'.repeat(2050)
	])
		assertEqual(validateRunnerUrl(bad), ERRORS.urlInvalid, bad);
	assertEqual(normalizeRunnerUrl(' https://r:9090/ '), 'https://r:9090', 'normalised');
});

suite.check('working directory: absolute, no . or .. segments, no //', () => {
	for (const ok of ['/a', '/home/you/work/project', '/a/b/', '/a/.hidden', '/a/b..c'])
		assertEqual(validateWorkingDirectory(ok), null, ok);
	assertEqual(validateWorkingDirectory('relative'), ERRORS.dirAbsolute, 'relative');
	assertEqual(validateWorkingDirectory(''), ERRORS.dirAbsolute, 'empty');
	assertEqual(validateWorkingDirectory('/'), ERRORS.dirAbsolute, 'root only');
	assertEqual(validateWorkingDirectory('/a\u0001b'), ERRORS.dirAbsolute, 'control character');
	assertEqual(validateWorkingDirectory('/' + 'a'.repeat(1024)), ERRORS.dirAbsolute, 'too long');
	for (const bad of ['/a/../b', '/a/./b', '/a//b', '/a/..', '/a//'])
		assertEqual(validateWorkingDirectory(bad), ERRORS.dirSegments, bad);
	assertEqual(normalizeDirectory(' /a/b/ '), '/a/b', 'one trailing slash dropped');
	assertEqual(normalizeDirectory('/'), '/', 'root kept');
});

suite.check('advanced: level, veto, reason length and domains', () => {
	const base = defaultDraft();
	const clean = validateAdvanced(base);
	assertEqual(clean.level, null, 'default level is valid');
	assertEqual(clean.reasonLength, null, 'default length is valid');
	assertEqual(validateAdvanced({ ...base, level: null }).level, ERRORS.level, 'no level');
	assertEqual(validateAdvanced({ ...base, veto: true }).veto, ERRORS.veto, 'veto below vetoer');
	assertEqual(
		validateAdvanced({ ...base, veto: true, level: 'vetoer' }).veto,
		null,
		'veto at vetoer'
	);
	assertEqual(
		validateAdvanced({ ...base, veto: true, level: 'autonomous' }).veto,
		null,
		'autonomous'
	);
	for (const bad of ['', '-1', '2001', 'ten', '1.5', '10000'])
		assertEqual(
			validateAdvanced({ ...base, reasonLength: bad }).reasonLength,
			ERRORS.reasonLength,
			bad
		);
	for (const ok of ['0', '2000', '50']) assertEqual(parseReasonLength(ok) !== null, true, ok);
	const dup = validateAdvanced({
		...base,
		domains: [
			{ domain: 'code', level: 'advisor' },
			{ domain: 'code', level: 'voter' },
			{ domain: null, level: 'voter' }
		]
	});
	assertEqual(
		dup.domains.join('|'),
		`|${ERRORS.domainDuplicate}|${ERRORS.domainChoose}`,
		'domains'
	);
});

// --- status chips ---------------------------------------------------------------------------------

suite.check('every state of the status table has its chip', () => {
	assertEqual(runnerChip('online').tone, 'success', 'online');
	assertEqual(runnerChip('waiting').tone, 'warning', 'waiting');
	assertEqual(runnerChip('not_reporting').tone, 'warning', 'not reporting');
	assertEqual(runnerChip('unreachable').tone, 'danger', 'unreachable');
	assertEqual(runnerChip('waiting').key, 'execution.connection.status.runner.waiting', 'key');
	const components: Array<[Parameters<typeof componentChip>[0], string]> = [
		['ready', 'success'],
		['warning', 'warning'],
		['missing', 'danger'],
		['disabled', 'danger'],
		['unknown', 'neutral']
	];
	for (const [state, tone] of components) assertEqual(componentChip(state).tone, tone, state);
	assertEqual(
		componentChip('disabled').key,
		'execution.connection.status.component.disabled',
		'disabled text'
	);
	const deliveries: Array<[Parameters<typeof deliveryChip>[0], string]> = [
		['delivered', 'success'],
		['refused', 'warning'],
		['failed', 'danger'],
		['none', 'neutral']
	];
	for (const [state, tone] of deliveries) assertEqual(deliveryChip(state).tone, tone, state);
});

suite.check('relative times, the delivery line and the polling window', () => {
	const now = new Date('2026-11-21T12:00:00Z');
	assertEqual(relativeTime('2026-11-21T11:55:00Z', now, 'en'), '5 minutes ago', 'minutes');
	assertEqual(relativeTime('2026-11-21T09:00:00Z', now, 'en'), '3 hours ago', 'hours');
	assertEqual(relativeTime(null, now, 'en'), '', 'none');
	assertEqual(relativeTime('nonsense', now, 'en'), '', 'unparseable');
	assertEqual(
		deliveryValue(
			{
				state: 'refused',
				at: '2026-11-21T11:55:00Z',
				project_key: 'SVWL',
				task_id: 't',
				reason_code: 'path_outside_allowlist'
			},
			now,
			'en'
		),
		'5 minutes ago · SVWL · path_outside_allowlist',
		'time · project · code'
	);
	assertEqual(
		deliveryValue(
			{ state: 'none', at: null, project_key: null, task_id: null, reason_code: null },
			now,
			'en'
		),
		'',
		'no delivery yet'
	);
	const status = parseStatus({
		runner: { state: 'waiting' },
		cli: { state: 'unknown' },
		tmux: null,
		last_delivery: { state: 'none' }
	});
	assertEqual(shouldPollWaiting(status, 0), true, 'waiting polls');
	assertEqual(shouldPollWaiting(status, 600), false, 'for at most ten minutes');
	const online = parseStatus({
		runner: { state: 'online' },
		cli: { state: 'ready' },
		last_delivery: { state: 'none' }
	});
	assertEqual(shouldPollWaiting(online, 0), false, 'online does not poll');
	assertEqual(shouldPollWaiting(null, 0), false, 'no status');
});

// --- answers are read at the boundary ----------------------------------------------------------

suite.check(
	'a connection is read strictly: a missing or malformed field is not a connection',
	() => {
		const parsed = parseConnection(connectionJson);
		assertEqual(parsed?.bot.name, 'Build bot', 'name');
		assertEqual(parsed?.governance.domain_overrides.code_review, 'advisor', 'overrides');
		const broken = (change: (copy: Record<string, unknown>) => void) => {
			const copy = JSON.parse(JSON.stringify(connectionJson)) as Record<string, unknown>;
			change(copy);
			return parseConnection(copy);
		};
		assertEqual(
			broken((c) => delete c.version),
			null,
			'no version'
		);
		assertEqual(
			broken((c) => (c.version = '3')),
			null,
			'string version'
		);
		assertEqual(
			broken((c) => ((c.execution as Record<string, unknown>).cli = 'vim')),
			null,
			'unknown CLI'
		);
		assertEqual(
			broken((c) => ((c.governance as Record<string, unknown>).max_domain_level = 'boss')),
			null,
			'unknown level'
		);
		assertEqual(
			broken((c) => ((c.runner as Record<string, unknown>).shared_with_projects = [1])),
			null,
			'projects'
		);
		assertEqual(parseConnection(null), null, 'null');
		assertEqual(parseConnection('x'), null, 'text');
	}
);

suite.check(
	'reads: none, full, reduced; writes carry the block; removal names what is left',
	() => {
		assertEqual(parseRead({ connection: null, status: null })?.kind, 'none', 'none');
		assertEqual(parseRead({ connection: connectionJson, status: null })?.kind, 'full', 'full');
		assertEqual(
			parseRead({
				connection: { bot: { id: 'b', name: 'x' }, enabled: true, cli: 'claude', channel: 'tmux' }
			})?.kind,
			'reduced',
			'a member sees the reduced view'
		);
		assertEqual(parseRead([]), null, 'not an object');
		const write = parseWrite({
			connection: connectionJson,
			status: null,
			config_block: { text: BLOCK_TEXT }
		});
		assertEqual(write?.block?.text, BLOCK_TEXT, 'the block');
		assertEqual(parseWrite({ connection: connectionJson })?.block, null, 'no block');
		assertEqual(parseWrite({ connection: 3 }), null, 'bad connection');
		assertEqual(parseBlock({ text: '' }), null, 'empty block is none');
		assertEqual(parseBlock(null), null, 'null');
		assertEqual(
			parseRemove({ removed: true, bot_still_connected_to: [{ id: 'p2', name: 'Other' }] })?.still
				.length,
			1,
			'still'
		);
		assertEqual(parseRemove({ removed: false, bot_still_connected_to: [] }), null, 'not removed');
	}
);

suite.check('the server refuses with `field: reason`; the reason lands under its field', () => {
	assertEqual(routeServerMessage('runner_url: blocked address').field, 'url', 'url');
	assertEqual(routeServerMessage('runner_url: blocked address').text, 'blocked address', 'text');
	assertEqual(
		routeServerMessage('bot.name: a bot with this name already exists in this workspace').field,
		'name',
		'name'
	);
	assertEqual(
		routeServerMessage('bot.id: not a connected bot of this workspace').field,
		'bot',
		'bot'
	);
	assertEqual(
		routeServerMessage('working_directory: must be an absolute path').field,
		'directory',
		'dir'
	);
	assertEqual(
		routeServerMessage('project already has a bot connection; reload').field,
		'other',
		'other'
	);
});

// --- the advanced draft ----------------------------------------------------------------------------

suite.check('the advanced draft round-trips and sends only what changed', () => {
	const connection = parseConnection(connectionJson);
	assert(connection !== null, 'parsed');
	const draft = draftOf(connection);
	assertEqual(draft.onComplete, 'state', 'on completion');
	assertEqual(draft.stateKey, 'done', 'state key');
	assertEqual(onCompleteValue(draft), 'state:done', 'value');
	assertEqual(Object.keys(changedFields(draft, connection)).length, 0, 'unchanged sends nothing');
	const changed = changedFields(
		{ ...draft, channel: 'headless', onComplete: 'none', level: 'vetoer', veto: true, domains: [] },
		connection
	);
	assertEqual(changed.channel, 'headless', 'channel');
	assertEqual(changed.on_complete_state, 'none', 'completion');
	assertEqual(changed.governance?.max_domain_level, 'vetoer', 'level');
	assertEqual(changed.governance?.can_veto_human_consensus, true, 'veto');
	assertEqual(
		Object.keys(changed.governance?.domain_overrides ?? { x: 1 }).length,
		0,
		'overrides cleared'
	);
	assertEqual(changed.governance?.reason_min_length, undefined, 'length untouched');
	const fields = connectFields(defaultDraft());
	assertEqual(fields.channel, 'tmux', 'default channel');
	assertEqual(fields.on_complete_state, 'auto', 'default completion');
	assertEqual(fields.governance?.max_domain_level, 'voter', 'default level');
	assertEqual(fields.governance?.reason_min_length, 50, 'default length');
	assertEqual(
		validateDraft({ ...draft, onComplete: 'state', stateKey: null }).state !== null,
		true,
		'a state must be chosen'
	);
});

// --- the one-time block ----------------------------------------------------------------------------

suite.check('the block state machine: shown, stored, done; text only while shown', () => {
	assertEqual(visibleText(NO_BLOCK), '', 'nothing shown');
	const shown = showBlock(BLOCK_TEXT);
	assertEqual(visibleText(shown), BLOCK_TEXT, 'text while shown');
	assertEqual(canFinish(shown), false, 'Done needs the confirmation');
	assertEqual(finishBlock(shown), shown, 'Done before the confirmation changes nothing');
	assertEqual(needsLeaveConfirmation(shown), true, 'leaving an unconfirmed block asks');
	const stored = setStored(shown, true);
	assertEqual(canFinish(stored), true, 'Done after the confirmation');
	assertEqual(needsLeaveConfirmation(stored), false, 'a confirmed block does not ask');
	assertEqual(setStored(stored, false).phase, 'shown', 'unchecking keeps it shown');
	const done = finishBlock(stored);
	assertEqual(done.phase, 'none', 'Done drops the block');
	assertEqual(JSON.stringify(done).includes(SECRET), false, 'no trace of the text after Done');
	assertEqual(JSON.stringify(discard()).includes(SECRET), false, 'no trace after leaving');
	assertEqual(showBlock('').phase, 'none', 'an empty block is no block');
	assertEqual(setStored(NO_BLOCK, true), NO_BLOCK, 'nothing to confirm');
	assertEqual(textareaRows('a'), 3, 'at least three rows');
	assertEqual(textareaRows('x\n'.repeat(60)), 24, 'at most 24 rows');
	const file = blockFile(BLOCK_TEXT);
	assertEqual(file.name, 'sylvode-runner.toml', 'file name');
	assertEqual(file.blob.size, new TextEncoder().encode(BLOCK_TEXT).length, 'built from memory');
});

await suite.checkAsync(
	'the block component shows the text while shown and Done waits for the confirmation',
	async () => {
		const props = { block: showBlock(BLOCK_TEXT), onstored: () => {}, ondone: () => {} };
		const html = await renderToHtml(ConfigBlock, props);
		assert(html.includes(SECRET), 'the text is in the textarea while shown');
		assert(html.includes('execution.connection.block.title'), 'the warning');
		assert(html.includes('execution.connection.block.scope'), 'the scope of the token is stated');
		assert(html.includes('execution.connection.block.doneReason'), 'Done says why it is disabled');
		const after = await renderToHtml(ConfigBlock, { ...props, block: NO_BLOCK });
		assertEqual(after.includes(SECRET), false, 'after Done the component renders no text');
	}
);

function sources(dir: string): Array<{ file: string; text: string }> {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return sources(path);
		return /\.(ts|svelte)$/.test(entry.name)
			? [{ file: path, text: readFileSync(path, 'utf8') }]
			: [];
	});
}

suite.check('the connection code never stores, logs or navigates with the block', () => {
	const files = [
		...sources('src/lib/project/connection'),
		{
			file: 'src/lib/api/bot-connection.ts',
			text: readFileSync('src/lib/api/bot-connection.ts', 'utf8')
		},
		...sources('src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/settings/execution')
	];
	assert(files.length >= 10, 'the sources were found');
	const forbidden: Array<[RegExp, string]> = [
		[/localStorage|sessionStorage|indexedDB|IndexedDB/, 'browser storage'],
		[/document\.cookie/, 'cookies'],
		[/console\.(log|info|warn|error|debug)/, 'the console'],
		[/workbench|WORKBENCH/, 'the workbench snapshot (it is persisted per tab)'],
		[/\.svelte\.ts['"]|writable\(|readable\(|\$app\/stores/, 'a store'],
		[
			/(?:goto|history\.(?:push|replace)State|searchParams)[^;\n]*(?:block|config)/i,
			'a URL that could carry the block'
		],
		[/\bfetch\(/, 'a request outside the API client']
	];
	for (const { file, text } of files)
		for (const [pattern, what] of forbidden)
			assert(!pattern.test(text.replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, '')), `${file} uses ${what}`);
});

suite.check(
	'no toast message is built from the block, and the copy control uses the shared helper',
	() => {
		const block = readFileSync('src/lib/project/connection/ConfigBlock.svelte', 'utf8');
		assert(block.includes("from '#lib/clipboard.js'"), 'the shared clipboard helper');
		assert(!/navigator\.clipboard|execCommand/.test(block), 'no private copy code');
		for (const call of block.match(/toast\.\w+\([^)]*\)/g) ?? [])
			assert(
				!/text|block|token|secret/i.test(call.replace(/execution\.connection\.block\.\w+/, '')),
				call
			);
		const section = readFileSync('src/lib/project/connection/ConnectionSection.svelte', 'utf8');
		for (const call of section.match(/toast\.\w+\([\s\S]*?\)\s*[;\n]/g) ?? [])
			assert(!/answer\.block|block\.text/.test(call), call);
		assert(!/auto.?download|\.click\(\)/.test(section), 'nothing downloads on its own');
		assert(/block\s*=\s*discard\(\)/.test(section), 'the block is dropped when the section goes');
		assert(section.includes('beforeNavigate') && section.includes('beforeunload'), 'leaving asks');
	}
);

suite.check('the API client keeps and caches nothing', () => {
	const text = readFileSync('src/lib/api/bot-connection.ts', 'utf8');
	assert(
		!/new Map|new WeakMap|cache|memo/i.test(text.replace(/\/\*[\s\S]*?\*\//g, '')),
		'no cache'
	);
	assert(!/(?:^|\s)(?:let|var)\s/m.test(text), 'no module state');
});

finish(suite.result());
