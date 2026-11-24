import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { authApi } from '../../src/lib/api/auth';
import { adminApi } from '../../src/lib/api/admin';
import {
	digestSwitchState,
	emailSwitchState,
	fromAnswer,
	languageOf,
	toApi,
	withDigest,
	withEmail,
	type PreferencesView
} from '../../src/lib/notifications/preferences';
import {
	changeContentText,
	parseChangeContent,
	valueLabel
} from '../../src/lib/notifications/change-text';
import { testEmailOutcome } from '../../src/lib/notifications/test-email';

const ROOT = new URL('../..', import.meta.url).pathname;
const read = (path: string) => readFileSync(join(ROOT, path), 'utf8');

const answer = (
	prefs: Partial<{ email: boolean; digest: boolean; mention: boolean }>,
	available = true
) => ({
	email_available: available,
	notification_prefs: {
		email_notification: prefs.email ?? true,
		daily_digest: prefs.digest ?? false,
		mention_only: prefs.mention ?? false,
		language: 'en' as const
	}
});

// Fake translation: the key, then the values in order, so a test sees what was passed.
const translate = (key: string, options?: { values?: Record<string, string | number> }) =>
	options?.values ? `${key} ${JSON.stringify(options.values)}` : key;

test('language follows the interface locale and is always sent', () => {
	assert.equal(languageOf('zh'), 'zh');
	assert.equal(languageOf('zh-CN'), 'zh');
	assert.equal(languageOf('en-US'), 'en');
	assert.equal(languageOf(null), 'en');
	const view = fromAnswer(answer({ email: true, digest: true }));
	assert.deepEqual(toApi(view.form, 'zh'), {
		email_notification: true,
		mention_only: false,
		daily_digest: true,
		language: 'zh'
	});
	assert.equal(toApi(view.form, undefined).language, 'en');
});

test('a daily summary needs e-mail on: reading, toggling and saving all enforce it', () => {
	// stored digest without e-mail reads as off
	assert.equal(fromAnswer(answer({ email: false, digest: true })).form.dailyDigest, false);
	const view = fromAnswer(answer({ email: true, digest: true }));
	const off = withEmail(view.form, false);
	assert.equal(off.dailyDigest, false);
	// turning e-mail on again does not bring the summary back
	assert.equal(withEmail(off, true).dailyDigest, false);
	// the summary cannot be switched on while e-mail is off
	assert.equal(withDigest(off, true).dailyDigest, false);
	assert.equal(withDigest(withEmail(off, true), true).dailyDigest, true);
	// and a form that somehow carries both is saved without the summary
	assert.equal(
		toApi({ emailNotification: false, dailyDigest: true, mentionOnly: true }, 'en').daily_digest,
		false
	);
});

test('mention only touches nothing else', () => {
	const form = fromAnswer(answer({ email: true, digest: true })).form;
	const changed = { ...form, mentionOnly: true };
	const saved = toApi(changed, 'en');
	assert.equal(saved.mention_only, true);
	assert.equal(saved.email_notification, true);
	assert.equal(saved.daily_digest, true);
});

test('without server e-mail both e-mail switches are disabled with the server notice', () => {
	const view: PreferencesView = fromAnswer(answer({ email: true, digest: true }, false));
	assert.equal(view.emailAvailable, false);
	assert.deepEqual(emailSwitchState(view), {
		disabled: true,
		reasonKey: 'notificationPrefs.emailUnavailable'
	});
	assert.equal(digestSwitchState(view).disabled, true);
	assert.equal(digestSwitchState(view).reasonKey, 'notificationPrefs.emailUnavailable');
});

test('with server e-mail the summary switch waits for the e-mail switch', () => {
	const on = fromAnswer(answer({ email: true }));
	assert.equal(emailSwitchState(on).disabled, false);
	assert.equal(digestSwitchState(on).disabled, false);
	const off = fromAnswer(answer({ email: false }));
	assert.equal(emailSwitchState(off).disabled, false);
	assert.deepEqual(digestSwitchState(off), {
		disabled: true,
		reasonKey: 'notificationPrefs.digestNeedsEmail'
	});
});

test('the preference and test e-mail routes', async () => {
	const calls: Array<{ method: string; url: string; body?: unknown }> = [];
	const client = apiClient as unknown as Record<string, unknown>;
	const reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
	const methods = ['get', 'post', 'put'];
	for (const method of methods) {
		client[method] = (url: string, body?: unknown) => {
			calls.push({ method, url, body });
			return Promise.resolve(reply);
		};
	}
	try {
		const prefs = toApi(fromAnswer(answer({})).form, 'zh');
		await authApi.getPreferences();
		await authApi.updatePreferences(prefs);
		await adminApi.sendTestEmail();
	} finally {
		// own properties of the shared client: remove them so the other suites see the real one
		for (const method of methods) delete client[method];
	}
	assert.deepEqual(
		calls.map((call) => `${call.method} ${call.url}`),
		[
			'get /api/v1/auth/me/preferences',
			'put /api/v1/auth/me/preferences',
			'post /api/v1/admin/email/test'
		]
	);
	const body = calls[1].body as { notification_prefs: Record<string, unknown> };
	assert.equal(body.notification_prefs.language, 'zh');
});

test('the change content is split from the right, so a title may contain |', () => {
	const parsed = parseChangeContent('issue_state_changed', 'issue_state_changed:A | B|todo|done');
	assert.deepEqual(parsed, {
		kind: 'issue_state_changed',
		title: 'A | B',
		old: 'todo',
		next: 'done'
	});
	assert.equal(
		parseChangeContent('issue_priority_changed', 'issue_priority_changed:T||high')?.old,
		''
	);
	assert.equal(parseChangeContent('issue_state_changed', 'issue_state_changed:no values'), null);
	assert.equal(parseChangeContent('issue_state_changed', 'issue_state_changed:x|only'), null);
	assert.equal(parseChangeContent('issue_state_changed', 'issue_priority_changed:a|b|c'), null);
	assert.equal(parseChangeContent('mentioned', 'issue_state_changed:a|b|c'), null);
});

test('inbox text names the item and both values; a malformed content has a fixed text', () => {
	assert.equal(
		changeContentText(
			'issue_state_changed',
			'issue_state_changed:Fix | login|todo|in_progress',
			translate
		),
		'notification.issueStateChangedContent {"title":"Fix | login","old":"issue.todo","new":"issue.inProgress"}'
	);
	assert.equal(
		changeContentText('issue_priority_changed', 'issue_priority_changed:T|low|urgent', translate),
		'notification.issuePriorityChangedContent {"title":"T","old":"issue.low","new":"issue.urgent"}'
	);
	assert.equal(
		changeContentText('issue_state_changed', 'garbage', translate),
		'notification.issueStateChangedFallback'
	);
	// a custom workflow state has no words of ours and is shown as written
	assert.equal(valueLabel('issue_state_changed', 'Code review', translate), 'Code review');
	assert.equal(valueLabel('issue_priority_changed', 'todo', translate), 'todo');
});

test('the test e-mail answer maps to queued, not set up, forbidden or failed', () => {
	const result = (
		code: number,
		error_code?: string,
		queued = false
	): ApiResult<{ queued: boolean }> => ({
		code,
		message: 'm',
		data: code === 0 ? { queued } : null,
		...(error_code ? { error_code } : {})
	});
	assert.deepEqual(testEmailOutcome(result(0, undefined, true)), { kind: 'queued' });
	assert.deepEqual(testEmailOutcome(result(409, 'feature_disabled')), { kind: 'not_set_up' });
	assert.deepEqual(testEmailOutcome(result(403)), { kind: 'forbidden' });
	assert.equal(testEmailOutcome(result(409, 'other')).kind, 'failed');
	assert.equal(testEmailOutcome(result(500)).kind, 'failed');
});

test('the screens use the modules, and every text they use exists in en and zh', () => {
	const settings = read('src/routes/(app)/settings/+page.svelte');
	assert.match(settings, /NotificationPreferences/);
	assert.ok(!/daily_digest|type="checkbox"/.test(settings), 'the old inline switches are gone');
	assert.match(read('src/routes/(app)/admin/settings/+page.svelte'), /TestEmailControl/);
	const inbox = read('src/routes/(app)/inbox/+page.svelte');
	assert.match(inbox, /changeContentText/);
	for (const lang of ['en', 'zh']) {
		const messages = JSON.parse(read(`src/lib/notifications/locale.${lang}.json`)) as {
			notificationPrefs: Record<string, unknown>;
			notification: Record<string, string>;
		};
		for (const key of [
			'title',
			'email',
			'digest',
			'digestNeedsEmail',
			'mentionOnly',
			'emailUnavailable',
			'save',
			'saved'
		])
			assert.ok(messages.notificationPrefs[key], `${lang} notificationPrefs.${key}`);
		for (const key of [
			'issueStateChangedTitle',
			'issueStateChangedContent',
			'issuePriorityChangedTitle',
			'issuePriorityChangedContent'
		])
			assert.ok(messages.notification[key], `${lang} notification.${key}`);
	}
});
