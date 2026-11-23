/**
 * The one-time credentials of an inbound source: shown, stored, finished only after the
 * confirmation, discarded, leave confirmation, and no state other than `shown` holds a value.
 *
 * Run standalone: `bun test tests/features/inbound-secrets-state.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { Suite, assertEqual } from '../support/harness';
import {
	NO_SECRETS,
	canFinish,
	credentialsJson,
	discard,
	finish as finishSecrets,
	guessedEndpoint,
	needsLeaveConfirmation,
	setStored,
	showSecrets,
	visibleValues
} from '../../src/lib/project/inbound/secrets-state';

const suite = new Suite('inbound-secrets-state');
const TOKEN = `svi_${'c3'.repeat(32)}`;
const SECRET = `svs_${'d4'.repeat(32)}`;
const values = {
	endpoint: 'https://sv.example/api/v1/inbound/events',
	token: TOKEN,
	signingSecret: SECRET
};
const leaks = (state: unknown) => {
	const text = JSON.stringify(state);
	return text.includes(TOKEN) || text.includes(SECRET);
};

suite.check('shown, stored, done; values only while shown', () => {
	assertEqual(visibleValues(NO_SECRETS), null, 'nothing shown');
	const shown = showSecrets(values);
	assertEqual(visibleValues(shown)?.token, TOKEN, 'values while shown');
	assertEqual(canFinish(shown), false, 'Done needs the confirmation');
	assertEqual(finishSecrets(shown), shown, 'Done before the confirmation changes nothing');
	assertEqual(needsLeaveConfirmation(shown), true, 'leaving unconfirmed asks');
	const stored = setStored(shown, true);
	assertEqual(canFinish(stored), true, 'Done after the confirmation');
	assertEqual(needsLeaveConfirmation(stored), false, 'a confirmed state does not ask');
	assertEqual(setStored(stored, false).phase, 'shown', 'unchecking keeps it shown');
	assertEqual(finishSecrets(stored).phase, 'none', 'Done drops the values');
});

suite.check('no state other than shown contains a value', () => {
	const shown = showSecrets(values);
	assertEqual(leaks(shown), true, 'the shown state is where they live');
	assertEqual(leaks(NO_SECRETS), false, 'none');
	assertEqual(leaks(finishSecrets(setStored(shown, true))), false, 'after Done');
	assertEqual(leaks(discard()), false, 'after leaving');
	assertEqual(leaks(setStored(NO_SECRETS, true)), false, 'confirming nothing');
	assertEqual(setStored(NO_SECRETS, true), NO_SECRETS, 'nothing to confirm');
	assertEqual(
		showSecrets({ endpoint: null, token: '', signingSecret: '' }).phase,
		'none',
		'empty is none'
	);
});

suite.check('copy all as JSON, and the endpoint fallback', () => {
	const parsed = JSON.parse(credentialsJson(values, 'https://fallback')) as Record<string, string>;
	assertEqual(parsed.url, values.endpoint, 'url');
	assertEqual(parsed.token, TOKEN, 'token');
	assertEqual(parsed.signing_secret, SECRET, 'signing secret');
	const guessed = JSON.parse(
		credentialsJson({ ...values, endpoint: null }, guessedEndpoint('https://host.example'))
	) as Record<string, string>;
	assertEqual(guessed.url, 'https://host.example/api/v1/inbound/events', 'fallback address');
});

test(suite.name, () => {
	const result = suite.result();
	nodeAssert.equal(result.failed, 0, result.failures.join('\n'));
});
