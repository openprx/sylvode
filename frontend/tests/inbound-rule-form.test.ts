/**
 * The alert rule form: kind patterns, validation, defaults (the action follows the severity until
 * touched), the full-body rule of every save (create, replace, enable switch), the reorder step,
 * server message routing, and the row and editor states of the rules section.
 *
 * Run standalone: `bun tests/inbound-rule-form.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { apiClient, type ApiResult } from '../src/lib/api/client';
import { inboundApi } from '../src/lib/api/inbound';
import {
	RULE_BODY_FIELDS,
	buildBody,
	buildReplaceBody,
	defaultActionFor,
	draftChanged,
	draftFromRule,
	draftValid,
	isKindPattern,
	limitsAreDefault,
	movedOrder,
	newDraft,
	parseKinds,
	routeServerMessage,
	ruleToBody,
	toggleBody,
	validateDraft,
	withAction,
	withSeverity
} from '../src/lib/project/inbound/rule-form';
import { toDuration } from '../src/lib/project/inbound/duration';
import { parseRule, type Rule } from '../src/lib/project/inbound/wire';

await installSvelteSsr();
const { default: RuleRow } = await import('../src/lib/project/inbound/RuleRow.svelte');
const { default: RuleEditor } = await import('../src/lib/project/inbound/RuleEditor.svelte');

const suite = new Suite('inbound-rule-form');

/** Every field differs from its default, so a dropped field changes the body. */
const stored = parseRule({
	id: 'r1',
	workspace_id: 'w',
	project_id: 'p',
	name: 'WAF high to triage',
	position: 3,
	enabled: true,
	source_id: 's1',
	kind_patterns: ['waf.attack_detected', 'waf.*'],
	min_severity: 'medium',
	action: 'work_item_and_task',
	bot_user_id: 'bot1',
	instructions: 'Look at {alert_identifier}',
	priority_map: { critical: 'high', high: 'medium', medium: 'low', low: 'urgent', info: 'medium' },
	label_ids: ['l1', 'l2'],
	notify_user_ids: ['u1', 'u2'],
	max_tasks_per_hour: 11,
	max_active_tasks: 4,
	dedup_window_secs: 90,
	rearm_after_secs: 0,
	task_max_attempts: 5,
	version: 7
});
assert(stored !== null, 'rule fixture');
const rule: Rule = stored;

const draft = () => newDraft({ currentUserId: 'me', connectedBotId: 'botC' });
const named = (name = 'r') => ({ ...draft(), name });

// --- kind patterns -------------------------------------------------------------------------------

suite.check('kind patterns: exact, prefix, and what is refused', () => {
	for (const ok of ['waf.attack_detected', 'waf.*', 'a', 'a.b.c.d', 'a.b.c.d.*', 'x_1.y_2'])
		assert(isKindPattern(ok), `${ok} is valid`);
	for (const bad of [
		'waf.',
		'WAF.x',
		'a.b.c.d.e',
		'*',
		'.*',
		'waf*',
		'a b',
		'a'.repeat(33),
		`${'a'.repeat(32)}.${'b'.repeat(32)}`, // 65 characters
		''
	])
		assert(!isKindPattern(bad), `${bad} is refused`);
	assert(isKindPattern(`${'a'.repeat(32)}.${'b'.repeat(31)}`), '64 characters pass');
});

suite.check('kind lines: duplicates dropped silently, blanks ignored, invalid reported', () => {
	const parsed = parseKinds('waf.a\n\n  waf.a  \nids.*\r\nBad.Kind\nwaf.a');
	assertDeepEqual(parsed.patterns, ['waf.a', 'ids.*'], 'patterns');
	assertDeepEqual(parsed.invalid, ['Bad.Kind'], 'invalid');
	const errors = validateDraft({ ...named(), kindsText: 'waf.\nok' });
	assertEqual(errors.kinds?.key, 'inbound.error.kindInvalid', 'message key');
	assertEqual(errors.kinds?.values?.pattern, 'waf.', 'names the pattern');
	const many = Array.from({ length: 33 }, (_, i) => `k${i}`).join('\n');
	assertEqual(
		validateDraft({ ...named(), kindsText: many }).kinds?.key,
		'inbound.error.kindCount',
		'33'
	);
	const max = Array.from({ length: 32 }, (_, i) => `k${i}`).join('\n');
	assertEqual(validateDraft({ ...named(), kindsText: max }).kinds, null, '32');
});

// --- defaults ------------------------------------------------------------------------------------

suite.check('new rule defaults: severity, recipient, connected bot, limits', () => {
	const d = draft();
	assertEqual(d.minSeverity, 'high', 'severity');
	assertEqual(d.action, 'work_item_and_task', 'action at high');
	assertDeepEqual(d.notifyIds, ['me'], 'the saver is notified');
	assertEqual(d.botId, 'botC', 'connected bot');
	assertEqual(d.enabled, true, 'enabled');
	assert(limitsAreDefault(d), 'limits are the defaults');
	assertEqual(d.dedupWindowSecs, 3600, 'one hour');
	assertEqual(d.rearmAfterSecs, 900, 'fifteen minutes');
	assertDeepEqual(
		d.priorityMap,
		{ critical: 'urgent', high: 'high', medium: 'medium', low: 'low', info: 'low' },
		'priorities'
	);
	assertEqual(newDraft({ currentUserId: null, connectedBotId: null }).botId, null, 'no bot');
	assertDeepEqual(newDraft({ currentUserId: null, connectedBotId: null }).notifyIds, [], 'nobody');
});

suite.check('the action follows the severity until touched, never after', () => {
	assertEqual(defaultActionFor('critical'), 'work_item_and_task', 'critical');
	assertEqual(defaultActionFor('medium'), 'work_item', 'medium');
	let d = withSeverity(draft(), 'medium');
	assertEqual(d.action, 'work_item', 'follows to medium');
	d = withSeverity(d, 'critical');
	assertEqual(d.action, 'work_item_and_task', 'follows back up');
	d = withAction(d, 'work_item');
	d = withSeverity(d, 'critical');
	assertEqual(d.action, 'work_item', 'touched: no longer follows');
	const edited = withSeverity(draftFromRule(rule), 'info');
	assertEqual(edited.action, 'work_item_and_task', 'a stored rule never follows');
});

// --- validation ----------------------------------------------------------------------------------

suite.check('validation: name, bot, instructions, counts, number ranges', () => {
	assertEqual(validateDraft(draft()).name?.key, 'inbound.error.nameRequired', 'empty name');
	assertEqual(
		validateDraft({ ...draft(), name: '   ' }).name?.key,
		'inbound.error.nameRequired',
		'blank'
	);
	assertEqual(
		validateDraft({ ...named('x'.repeat(101)) }).name?.key,
		'inbound.error.nameLength',
		'101'
	);
	assertEqual(validateDraft(named('x'.repeat(100))).name, null, '100');
	assertEqual(
		validateDraft({ ...named(), botId: null }).bot?.key,
		'inbound.error.botChoose',
		'bot needed'
	);
	assertEqual(
		validateDraft({ ...withAction(named(), 'work_item'), botId: null }).bot,
		null,
		'not needed'
	);
	assertEqual(
		validateDraft({ ...named(), instructions: 'x'.repeat(4001) }).instructions?.key,
		'inbound.error.instructionsLength',
		'4001'
	);
	assertEqual(
		validateDraft({ ...named(), instructions: 'x'.repeat(4000) }).instructions,
		null,
		'4000'
	);
	const ids = (n: number) => Array.from({ length: n }, (_, i) => `i${i}`);
	assertEqual(
		validateDraft({ ...named(), notifyIds: ids(51) }).notify?.key,
		'inbound.error.recipientsCount',
		'51 people'
	);
	assertEqual(validateDraft({ ...named(), notifyIds: ids(50) }).notify, null, '50 people');
	assertEqual(
		validateDraft({ ...named(), labelIds: ids(51) }).labels?.key,
		'inbound.error.labelsCount',
		'51 labels'
	);
	const range = (field: 'tasksPerHour' | 'activeTasks' | 'attempts', low: number, high: number) => {
		const errorOf = (text: string) => validateDraft({ ...named(), [field]: text })[field];
		assertEqual(errorOf(String(low)), null, `${field} low`);
		assertEqual(errorOf(String(high)), null, `${field} high`);
		for (const bad of [String(low - 1), String(high + 1), '', 'x', '1.5', '-1'])
			assertEqual(errorOf(bad)?.key, 'inbound.error.range', `${field} ${bad}`);
		assertDeepEqual(errorOf('0')?.values ?? null, { min: low, max: high }, `${field} values`);
	};
	range('tasksPerHour', 1, 120);
	range('activeTasks', 1, 20);
	range('attempts', 1, 10);
	const dedup = (s: number | null) => validateDraft({ ...named(), dedupWindowSecs: s }).dedupWindow;
	assertEqual(dedup(60), null, '60 s');
	assertEqual(dedup(7 * 86400), null, '7 days');
	assert(
		dedup(59) !== null && dedup(7 * 86400 + 1) !== null && dedup(null) !== null,
		'dedup out of range'
	);
	const rearm = (s: number | null) => validateDraft({ ...named(), rearmAfterSecs: s }).rearm;
	assertEqual(rearm(0), null, '0');
	assertEqual(rearm(86400), null, '1 day');
	assert(rearm(86401) !== null && rearm(null) !== null, 'rearm out of range');
	assert(draftValid(validateDraft(named())), 'a named default draft is valid');
});

suite.check('duration unit choice never changes an untouched value', () => {
	for (const seconds of [3600, 90, 60, 900, 7200, 86400]) {
		const d = { ...named(), dedupWindowSecs: seconds, rearmAfterSecs: seconds };
		const duration = toDuration(seconds);
		const factor = { seconds: 1, minutes: 60, hours: 3600 }[duration.unit];
		assertEqual(duration.amount * factor, seconds, `${seconds} survives the unit`);
		const body = buildBody(d);
		assertEqual(body?.dedup_window_secs, seconds, `${seconds} sent`);
		assertEqual(body?.rearm_after_secs, seconds, `${seconds} rearm sent`);
	}
	assertEqual(toDuration(3600).unit, 'hours', '3600 is an hour');
	assertEqual(toDuration(90).unit, 'seconds', '90 stays in seconds');
});

// --- full bodies ---------------------------------------------------------------------------------

const sorted = (keys: readonly string[]) => [...keys].sort();
const FIELDS = [...RULE_BODY_FIELDS, 'bot_user_id'];

suite.check('create: every field is stated, including action, recipients and priorities', () => {
	const body = buildBody({ ...named('  new rule  '), notifyIds: [] });
	assert(body !== null, 'valid');
	assertDeepEqual(sorted(Object.keys(body)), sorted(FIELDS), 'all fields (hand to bot)');
	assertEqual(body.name, 'new rule', 'trimmed');
	assertEqual(body.action, 'work_item_and_task', 'action stated');
	assertDeepEqual(body.notify_user_ids, [], 'an empty recipient list is stated, not omitted');
	assertEqual(body.priority_map.critical, 'urgent', 'priority map stated');
	assertEqual(body.bot_user_id, 'botC', 'bot with hand to bot');
	assertEqual(body.source_id, null, 'any source is an explicit null');
	assertDeepEqual(body.kind_patterns, [], 'no kinds is an explicit empty list');
	assertEqual(body.max_tasks_per_hour, 6, 'defaults stated');
});

suite.check('bot_user_id is sent only with "hand to bot"; instructions always', () => {
	const open = buildBody({ ...withAction(named(), 'work_item'), instructions: 'typed earlier' });
	assert(open !== null, 'valid');
	assert(!('bot_user_id' in open), 'no bot with open-only');
	assertEqual(open.instructions, 'typed earlier', 'instructions kept and sent');
	assertDeepEqual(sorted(Object.keys(open)), sorted(RULE_BODY_FIELDS), 'all other fields');
	assertEqual(buildBody({ ...named(), botId: null }), null, 'invalid draft builds nothing');
});

suite.check(
	'replace: an unedited editor save sends the stored rule unchanged, plus the version',
	() => {
		const body = buildReplaceBody(draftFromRule(rule), rule);
		assert(body !== null, 'valid');
		assertDeepEqual(
			body,
			{
				name: 'WAF high to triage',
				enabled: true,
				source_id: 's1',
				kind_patterns: ['waf.attack_detected', 'waf.*'],
				min_severity: 'medium',
				action: 'work_item_and_task',
				bot_user_id: 'bot1',
				instructions: 'Look at {alert_identifier}',
				priority_map: rule.priority_map,
				label_ids: ['l1', 'l2'],
				notify_user_ids: ['u1', 'u2'],
				max_tasks_per_hour: 11,
				max_active_tasks: 4,
				dedup_window_secs: 90,
				rearm_after_secs: 0,
				task_max_attempts: 5,
				expected_version: 7
			},
			'whole rule'
		);
		assert(!draftChanged(draftFromRule(rule), draftFromRule(rule)), 'no change');
	}
);

suite.check('enable switch: the whole rule with enabled flipped, never a partial body', () => {
	const off = toggleBody(rule, false);
	assertDeepEqual(
		sorted(Object.keys(off)),
		sorted([...FIELDS, 'expected_version']),
		'every field present'
	);
	assertEqual(off.enabled, false, 'flipped');
	assertEqual(off.expected_version, 7, 'version');
	const without = (body: object, ...omit: string[]) =>
		Object.fromEntries(Object.entries(body).filter(([key]) => !omit.includes(key)));
	const rest = without(off, 'enabled', 'expected_version');
	const same = without(ruleToBody(rule), 'enabled');
	assertDeepEqual(rest, same, 'nothing else changed');
	// A rule that opens a work item only carries no bot.
	const open = toggleBody({ ...rule, action: 'work_item', bot_user_id: null }, true);
	assert(!('bot_user_id' in open), 'no bot for open-only');
	assertDeepEqual(
		sorted(Object.keys(open)),
		sorted([...RULE_BODY_FIELDS, 'expected_version']),
		'rest present'
	);
});

suite.check('every field of the stored rule reaches the body (a dropped field fails here)', () => {
	const body = ruleToBody(rule) as unknown as Record<string, unknown>;
	const wire: Record<string, unknown> = {
		name: rule.name,
		enabled: rule.enabled,
		source_id: rule.source_id,
		kind_patterns: rule.kind_patterns,
		min_severity: rule.min_severity,
		action: rule.action,
		bot_user_id: rule.bot_user_id,
		instructions: rule.instructions,
		priority_map: rule.priority_map,
		label_ids: rule.label_ids,
		notify_user_ids: rule.notify_user_ids,
		max_tasks_per_hour: rule.max_tasks_per_hour,
		max_active_tasks: rule.max_active_tasks,
		dedup_window_secs: rule.dedup_window_secs,
		rearm_after_secs: rule.rearm_after_secs,
		task_max_attempts: rule.task_max_attempts
	};
	for (const [key, value] of Object.entries(wire)) assertDeepEqual(body[key], value, `body.${key}`);
	for (const field of FIELDS) assert(field in body, `${field} stated`);
});

type Call = { method: string; url: string; body?: unknown };
const calls: Call[] = [];
let reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
const client = apiClient as unknown as Record<string, unknown>;
for (const method of ['get', 'post', 'put', 'patch', 'delete']) {
	client[method] = (url: string, body?: unknown) => {
		calls.push({ method, url, body });
		return Promise.resolve(reply);
	};
}

await suite.checkAsync('the requests of the three kinds of save carry the whole rule', async () => {
	calls.length = 0;
	reply = { code: 0, message: 'ok', data: { rule: { ...rule }, warnings: [] } };
	const created = buildBody(named('n'));
	const edited = buildReplaceBody({ ...draftFromRule(rule), name: 'renamed' }, rule);
	assert(created !== null && edited !== null, 'bodies');
	await inboundApi.createRule('p1', created);
	await inboundApi.replaceRule('r1', edited);
	await inboundApi.replaceRule('r1', toggleBody(rule, false));
	assertEqual(
		calls.map((c) => `${c.method} ${c.url}`).join('|'),
		'post /api/v1/projects/p1/alert-rules|put /api/v1/alert-rules/r1|put /api/v1/alert-rules/r1',
		'routes'
	);
	const keys = (call: Call | undefined) => sorted(Object.keys((call?.body ?? {}) as object));
	assertDeepEqual(keys(calls[0]), sorted(FIELDS), 'create body');
	assertDeepEqual(keys(calls[1]), sorted([...FIELDS, 'expected_version']), 'save body');
	assertDeepEqual(keys(calls[2]), sorted([...FIELDS, 'expected_version']), 'switch body');
	assertEqual((calls[1]?.body as { name: string }).name, 'renamed', 'edit applied');
});

await suite.checkAsync('a stale version is the typed conflict result', async () => {
	reply = { code: 409, message: 'conflict', data: null, error_code: 'version_conflict' };
	const result = await inboundApi.replaceRule('r1', toggleBody(rule, false));
	assert(!result.ok && result.kind === 'conflict', 'conflict');
});

// --- reorder -------------------------------------------------------------------------------------

suite.check('reorder: one step, the full order, nothing at the ends', () => {
	const ids = ['a', 'b', 'c'];
	assertDeepEqual(movedOrder(ids, 'b', -1), ['b', 'a', 'c'], 'up');
	assertDeepEqual(movedOrder(ids, 'b', 1), ['a', 'c', 'b'], 'down');
	assertEqual(movedOrder(ids, 'a', -1), null, 'first stays');
	assertEqual(movedOrder(ids, 'c', 1), null, 'last stays');
	assertEqual(movedOrder(ids, 'zz', 1), null, 'unknown');
	assertDeepEqual(ids, ['a', 'b', 'c'], 'input untouched');
});

await suite.checkAsync('reorder request carries the whole new order', async () => {
	calls.length = 0;
	reply = { code: 0, message: 'ok', data: [] };
	await inboundApi.reorderRules('p1', movedOrder(['a', 'b', 'c'], 'c', -1) ?? []);
	assertDeepEqual(calls[0]?.body, { rule_ids: ['a', 'c', 'b'] }, 'full order');
});

// --- server messages -----------------------------------------------------------------------------

suite.check('server messages name their field first', () => {
	assertEqual(routeServerMessage('name must be 1 to 100 characters'), 'name', 'name');
	assertEqual(routeServerMessage('kind_patterns entry "x" is invalid'), 'kinds', 'kinds');
	assertEqual(routeServerMessage('bot_user_id is required'), 'bot', 'bot');
	assertEqual(routeServerMessage('notify_user_ids entry is not a member'), 'notify', 'notify');
	assertEqual(routeServerMessage('dedup_window_secs out of range'), 'dedupWindow', 'dedup');
	assertEqual(routeServerMessage('something else entirely'), 'other', 'other');
	assertEqual(routeServerMessage(''), 'other', 'empty');
});

// --- components ----------------------------------------------------------------------------------

const rowProps = (over: Record<string, unknown> = {}) => ({
	rule,
	index: 1,
	count: 3,
	sourceName: 'prx-waf edge-01',
	botName: 'triage-bot',
	warnBotNotConnected: false,
	warnNoRecipients: false,
	busy: false,
	ontoggle: async () => true,
	onmove: () => {},
	onedit: () => {},
	ondelete: () => {},
	...over
});

await suite.checkAsync('rule row: two lines, chip, warnings only when they apply', async () => {
	const html = await renderToHtml(RuleRow, rowProps());
	assert(
		html.includes('WAF high to triage') && html.includes('prx-waf edge-01'),
		'name and source'
	);
	assert(html.includes('waf.attack_detected, waf.*'), 'kinds');
	assert(html.includes('inbound.rules.actionBot'), 'hand to bot');
	assert(
		html.includes('inbound.rules.enabled') && html.includes('data-tone="success"'),
		'enabled chip'
	);
	assert(!html.includes('rule-warn-bot') && !html.includes('rule-warn-recipients'), 'no warnings');
	const warned = await renderToHtml(
		RuleRow,
		rowProps({ warnBotNotConnected: true, warnNoRecipients: true })
	);
	assert(
		warned.includes('rule-warn-bot') && warned.includes('rule-warn-recipients'),
		'both warnings'
	);
	const more = await renderToHtml(
		RuleRow,
		rowProps({ rule: { ...rule, kind_patterns: ['a', 'b', 'c', 'd'] } })
	);
	assert(more.includes('a, b') && more.includes('inbound.rules.moreKinds'), 'two kinds and +n');
	const open = await renderToHtml(
		RuleRow,
		rowProps({
			rule: { ...rule, enabled: false, action: 'work_item', source_id: null, kind_patterns: [] },
			sourceName: null
		})
	);
	assert(
		open.includes('inbound.rules.anySource') && open.includes('inbound.rules.anyKind'),
		'any source, any kind'
	);
	assert(
		open.includes('inbound.rules.actionWorkItem') && open.includes('inbound.rules.disabled'),
		'open only, disabled'
	);
});

await suite.checkAsync(
	'rule row: move buttons are disabled at the ends with the reason as name',
	async () => {
		const first = await renderToHtml(RuleRow, rowProps({ index: 0 }));
		assert(first.includes('inbound.rules.first'), 'first row says already first');
		const last = await renderToHtml(RuleRow, rowProps({ index: 2 }));
		assert(last.includes('inbound.rules.last'), 'last row says already last');
		const middle = await renderToHtml(RuleRow, rowProps());
		assert(
			middle.includes('inbound.rules.moveUp') && middle.includes('inbound.rules.moveDown'),
			'middle'
		);
	}
);

await suite.checkAsync(
	'rule editor: new rule opens on High with hand to bot; an edit shows the rule',
	async () => {
		const props = {
			projectId: 'p1',
			sources: [],
			bots: [{ id: 'botC', name: 'triage-bot' }],
			people: [{ id: 'me', name: 'Me' }],
			labels: [],
			currentUserId: 'me',
			connectedBotId: 'botC',
			connectionKnown: true,
			onsaved: () => {},
			oncancel: () => {},
			onreload: () => {}
		};
		const fresh = await renderToHtml(RuleEditor, { ...props, rule: null });
		assert(fresh.includes('inbound.rules.new'), 'new heading');
		assert(fresh.includes('inbound.rules.instructions'), 'instructions with hand to bot');
		assert(
			fresh.includes('inbound.rules.groupLimits') && fresh.includes('inbound.rules.limitsDefault'),
			'limits default'
		);
		const edit = await renderToHtml(RuleEditor, {
			...props,
			rule: { ...rule, action: 'work_item', bot_user_id: null }
		});
		assert(edit.includes('inbound.rules.editTitle'), 'edit heading');
		assert(!edit.includes('inbound.rules.instructionsHelp'), 'instructions hidden with open-only');
		assert(edit.includes('inbound.rules.limitsChanged'), 'changed limits are marked');
	}
);

finish(suite.result());
