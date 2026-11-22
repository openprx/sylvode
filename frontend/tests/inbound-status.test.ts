/**
 * Status derivations and word mappings of the inbound sections, the duration unit rule, the
 * shared components' states, and the locale files' placeholders.
 *
 * Run standalone: `bun tests/inbound-status.test.ts`
 */
import { readFileSync } from 'node:fs';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import {
	actingUserName,
	botNotConnected,
	eventWorkItemHref,
	hasNoRecipients,
	isSuspended,
	outcomeTone,
	outcomeWord,
	severityKey,
	severityOfRank,
	severityTone,
	sourceChip,
	sourceStatus,
	testAlertWord,
	testCauseWord,
	testReasonWord,
	testTaskWord
} from '../src/lib/project/inbound/status';
import { changeUnit, toDuration, toSeconds } from '../src/lib/project/inbound/duration';
import { loadStateOf } from '../src/lib/project/inbound/load-state';
import { parseEvent, parseRule, parseSource } from '../src/lib/project/inbound/wire';
import type { Connection } from '../src/lib/project/connection/wire';

await installSvelteSsr();
const { default: SeverityChip } = await import('../src/lib/project/inbound/SeverityChip.svelte');
const { default: SectionFrame } = await import('../src/lib/project/inbound/SectionFrame.svelte');

const suite = new Suite('inbound-status');
const NOW = new Date('2026-11-22T12:00:00Z');

const base = parseSource({
	id: 's1',
	workspace_id: 'w',
	project_id: 'p',
	name: 'n',
	token_prefix: 'svi_1',
	require_signature: true,
	has_signing_secret: true,
	ip_allowlist: [],
	acting_user_id: 'u1',
	enabled: true,
	rate_per_minute: 1,
	daily_cap: 1,
	last_used_at: null,
	last_used_ip: null,
	created_at: 'x',
	updated_at: 'x'
});
assert(base !== null, 'fixture');
const admins = [{ user_id: 'u1', name: 'Ada', role: 'admin' }];

suite.check('source status: waiting, receiving, idle, paused, suspended wins', () => {
	assertEqual(sourceStatus(base, NOW, admins), 'waiting', 'never used');
	assertEqual(
		sourceStatus({ ...base, last_used_at: '2026-11-22T11:00:00Z' }, NOW, admins),
		'receiving',
		'1 h ago'
	);
	assertEqual(
		sourceStatus({ ...base, last_used_at: '2026-11-21T12:00:00Z' }, NOW, admins),
		'receiving',
		'exactly 24 h'
	);
	assertEqual(
		sourceStatus({ ...base, last_used_at: '2026-11-21T11:59:59Z' }, NOW, admins),
		'idle',
		'older than 24 h'
	);
	assertEqual(sourceStatus({ ...base, enabled: false }, NOW, admins), 'paused', 'disabled');
	const used = { ...base, enabled: false, last_used_at: '2026-11-22T11:00:00Z' };
	assertEqual(
		sourceStatus({ ...used, suspended: true }, NOW, admins),
		'suspended',
		'suspended beats paused'
	);
	assertEqual(sourceChip('receiving').tone, 'success', 'receiving tone');
	assertEqual(sourceChip('suspended').tone, 'danger', 'suspended tone');
	assertEqual(sourceChip('paused').key, 'inbound.sources.status.paused', 'key');
});

suite.check('suspended: the server verdict (G3) wins, else the members list', () => {
	assertEqual(
		isSuspended({ ...base, suspended: false }, [{ user_id: 'u1', name: 'A', role: 'member' }]),
		false,
		'server says no'
	);
	assertEqual(isSuspended({ ...base, suspended: true }, admins), true, 'server says yes');
	assertEqual(isSuspended(base, admins), false, 'admin acting user');
	assertEqual(
		isSuspended(base, [{ user_id: 'u1', name: 'A', role: 'owner' }]),
		false,
		'owner acting user'
	);
	assertEqual(isSuspended(base, [{ user_id: 'u1', name: 'A', role: 'member' }]), true, 'demoted');
	assertEqual(isSuspended(base, []), true, 'removed');
	assertEqual(isSuspended({ ...base, acting_user_id: null }, admins), true, 'no acting user');
	assertEqual(isSuspended(base, null), false, 'unknown without either');
	assertEqual(actingUserName(base, admins), 'Ada', 'name from the list');
	assertEqual(
		actingUserName({ ...base, acting_user_name: 'Grace' }, admins),
		'Grace',
		'name from the server'
	);
	assertEqual(actingUserName(base, []), null, 'removed user');
});

suite.check('severity, outcome, rule test words; unknown codes print the code', () => {
	assertEqual(severityOfRank(4), 'critical', 'rank 4');
	assertEqual(severityOfRank(0), 'info', 'rank 0');
	assertEqual(severityOfRank(9), null, 'unknown rank');
	assertEqual(severityOfRank(1.5), null, 'fraction');
	assertEqual(severityTone('critical'), 'danger', 'critical');
	assertEqual(severityTone('high'), 'warning', 'high');
	for (const s of ['medium', 'low', 'info', null] as const)
		assertEqual(severityTone(s), 'neutral', String(s));
	assertEqual(severityKey(null), 'inbound.severity.unknown', 'unknown key');
	assertEqual(outcomeWord('alert_opened').key, 'inbound.outcome.alert_opened', 'known outcome');
	const unknown = outcomeWord('brand_new');
	assertEqual(unknown.key, null, 'no key');
	assertEqual(unknown.code, 'brand_new', 'the code itself');
	assertEqual(outcomeTone('bot_unavailable'), 'warning', 'warning outcome');
	assertEqual(outcomeTone('suppressed_alert_cap'), 'warning', 'warning outcome');
	assertEqual(outcomeTone('alert_opened'), 'neutral', 'neutral outcome');
	assertEqual(testAlertWord('would_open').key, 'inbound.test.alert.would_open', 'alert');
	assertEqual(testTaskWord('rearm_delay').key, 'inbound.test.task.rearm_delay', 'task');
	assertEqual(testCauseWord('queue_ahead').key, 'inbound.test.cause.queue_ahead', 'cause');
	assertEqual(
		testReasonWord('below_minimum_severity').key,
		'inbound.test.reason.below_minimum_severity',
		'reason'
	);
	assertEqual(testReasonWord('new_reason').code, 'new_reason', 'unknown reason');
});

suite.check('event work item link', () => {
	const event = parseEvent({
		id: 'e',
		event_id: 'x',
		kind: 'k',
		severity_rank: 1,
		dedup_key: 'd',
		occurred_at: 'a',
		received_at: 'b',
		title: '',
		summary: '',
		subject: null,
		payload: null,
		signed: false,
		client_ip: null,
		outcome: 'no_rule',
		rule_id: null,
		alert_id: null,
		task_id: null,
		work_item: { id: 'i 1', identifier: 'OPS-1' }
	});
	assert(event !== null, 'fixture');
	assertEqual(eventWorkItemHref('w', 'p', event), '/workspace/w/projects/p/issues/i%201', 'href');
	assertEqual(eventWorkItemHref('w', 'p', { ...event, work_item: null }), null, 'none');
});

const rule = parseRule({
	id: 'r',
	workspace_id: 'w',
	project_id: 'p',
	name: 'n',
	position: 1,
	enabled: true,
	source_id: null,
	kind_patterns: [],
	min_severity: 'high',
	action: 'work_item_and_task',
	bot_user_id: 'b1',
	instructions: '',
	priority_map: { critical: 'urgent', high: 'high', medium: 'medium', low: 'low', info: 'low' },
	label_ids: [],
	notify_user_ids: ['u1'],
	max_tasks_per_hour: 6,
	max_active_tasks: 2,
	dedup_window_secs: 3600,
	rearm_after_secs: 900,
	task_max_attempts: 3,
	version: 1
});
assert(rule !== null, 'rule fixture');
const connection = (enabled: boolean, bot: string) =>
	({ enabled, bot: { id: bot } }) as unknown as Connection;

suite.check('the "bot not connected" warning from the connection answer', () => {
	assertEqual(botNotConnected(rule, null), true, 'no connection');
	assertEqual(botNotConnected(rule, connection(false, 'b1')), true, 'disabled');
	assertEqual(botNotConnected(rule, connection(true, 'b2')), true, 'another bot');
	assertEqual(botNotConnected(rule, connection(true, 'b1')), false, 'same bot enabled');
	assertEqual(
		botNotConnected({ ...rule, action: 'work_item', bot_user_id: null }, null),
		false,
		'open-only rule'
	);
	assertEqual(hasNoRecipients(rule, new Set(['u1'])), false, 'recipient present');
	assertEqual(hasNoRecipients(rule, new Set(['u2'])), true, 'recipient gone');
});

suite.check('duration unit: the largest exact unit, never changing an untouched value', () => {
	assertEqual(JSON.stringify(toDuration(3600)), '{"amount":1,"unit":"hours"}', '3600');
	assertEqual(JSON.stringify(toDuration(90)), '{"amount":90,"unit":"seconds"}', '90');
	assertEqual(JSON.stringify(toDuration(900)), '{"amount":15,"unit":"minutes"}', '900');
	assertEqual(JSON.stringify(toDuration(0)), '{"amount":0,"unit":"seconds"}', '0');
	assertEqual(JSON.stringify(toDuration(604800)), '{"amount":168,"unit":"hours"}', '7 days');
	for (const secs of [0, 60, 90, 900, 3600, 5400, 86400, 604800]) {
		const d = toDuration(secs);
		assertEqual(toSeconds(d.amount, d.unit), secs, `round trip ${secs}`);
	}
	assertEqual(toSeconds(1.5, 'hours'), null, 'fractions refused');
	assertEqual(toSeconds(-1, 'seconds'), null, 'negatives refused');
	assertEqual(
		JSON.stringify(changeUnit({ amount: 1, unit: 'hours' }, 'minutes')),
		'{"amount":60,"unit":"minutes"}',
		'exact change'
	);
	assertEqual(
		JSON.stringify(changeUnit({ amount: 90, unit: 'seconds' }, 'minutes')),
		'{"amount":90,"unit":"minutes"}',
		'inexact change keeps the number'
	);
});

suite.check('load state: 403 is forbidden, other failures are errors', () => {
	assertEqual(loadStateOf({ ok: true, value: 1 }), 'ready', 'ok');
	assertEqual(
		loadStateOf({ ok: false, kind: 'error', code: 403, message: '', errorCode: null }),
		'forbidden',
		'403'
	);
	assertEqual(
		loadStateOf({ ok: false, kind: 'error', code: 500, message: '', errorCode: null }),
		'error',
		'500'
	);
	assertEqual(
		loadStateOf({ ok: false, kind: 'conflict', code: 409, message: '' }),
		'error',
		'conflict'
	);
});

await suite.checkAsync('components: severity chip words and the section frame states', async () => {
	const chip = await renderToHtml(SeverityChip, { severity: 'critical' });
	assert(
		chip.includes('inbound.severity.critical') && chip.includes('data-tone="danger"'),
		'critical chip'
	);
	const ranked = await renderToHtml(SeverityChip, { severity: 3 });
	assert(
		ranked.includes('inbound.severity.high') && ranked.includes('data-tone="warning"'),
		'rank 3'
	);
	const odd = await renderToHtml(SeverityChip, { severity: 42 });
	assert(
		odd.includes('inbound.severity.unknown') && odd.includes('data-tone="neutral"'),
		'unknown rank'
	);
	const frame = async (state: string, extra: Record<string, unknown> = {}) =>
		renderToHtml(SectionFrame, {
			titleKey: 'inbound.sources.title',
			descriptionKey: 'inbound.sources.description',
			thingKey: 'inbound.sources.thing',
			state,
			onretry: () => {},
			children: (() => '<p>BODY</p>') as never,
			...extra
		});
	const loading = await frame('loading');
	assert(
		loading.includes('inbound.sources.title') && loading.includes('data-kind="loading"'),
		'loading'
	);
	assert((await frame('error')).includes('data-kind="loadFailed"'), 'failed');
	const forbidden = await frame('forbidden');
	assert(
		forbidden.includes('data-kind="forbidden"') && forbidden.includes('inbound.forbidden'),
		'forbidden'
	);
	assert(
		!(await frame('ready')).includes('inbound.disabledOnServer'),
		'no tip bar when inbound is on'
	);
	assert(
		(await frame('ready', { inboundEnabled: false })).includes('inbound.disabledOnServer'),
		'G1 tip bar'
	);
});

suite.check('locale files: placeholders are plain {name} words the stand-in i18n can fill', () => {
	for (const lang of ['en', 'zh']) {
		const text = readFileSync(
			new URL(`../src/lib/project/inbound/locale.${lang}.json`, import.meta.url),
			'utf8'
		);
		assert(!/\{\s*\}/.test(text), `${lang}: empty placeholder`);
		assert(!text.includes("'{"), `${lang}: ICU quoting is not used by this i18n`);
	}
});

finish(suite.result());
