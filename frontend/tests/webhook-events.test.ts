/**
 * The webhook form offers every event the API accepts, and labels each one in every locale.
 *
 * The list is `WEBHOOK_EVENTS` in `src/lib/webhooks/events.ts`; the API's own test
 * (`frontend_webhook_events_match_the_api_list` in `apps/api/src/entities/webhook.rs`) holds it
 * equal to the server's list. This suite repeats that comparison so a frontend-only run catches
 * drift too, checks the copy in every locale, and checks that the page renders from the constant
 * instead of a list of its own.
 *
 * Run standalone: `bun tests/webhook-events.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from './support/harness';
import {
	WEBHOOK_EVENTS,
	WEBHOOK_EVENT_GROUPS,
	webhookEventGroupKey,
	webhookEventLabelKey
} from '../src/lib/webhooks/events';

const FRONTEND = new URL('..', import.meta.url).pathname;
const REPO = join(FRONTEND, '..');
const LOCALES = ['zh', 'en'] as const;
const PAGE = join(FRONTEND, 'src/routes/(app)/workspace/[workspaceId]/webhooks/+page.svelte');

const suite = new Suite('webhook-events');

function apiEvents(): string[] {
	const source = readFileSync(join(REPO, 'apps/api/src/entities/webhook.rs'), 'utf8');
	const list = source.match(/pub const WEBHOOK_EVENTS: &\[&str\] = &\[([\s\S]*?)\];/);
	assert(list, 'apps/api/src/entities/webhook.rs no longer defines WEBHOOK_EVENTS as a literal list');
	return [...list[1].matchAll(/"([a-z_.]+)"/g)].map((match) => match[1]);
}

function lookup(tree: unknown, key: string): unknown {
	return key.split('.').reduce<unknown>((node, part) => {
		if (node && typeof node === 'object' && part in node) {
			return (node as Record<string, unknown>)[part];
		}
		return undefined;
	}, tree);
}

suite.check('the form offers all events the API accepts, in its order', () => {
	assertDeepEqual([...WEBHOOK_EVENTS], apiEvents(), 'frontend vs apps/api/src/entities/webhook.rs');
	assert(WEBHOOK_EVENTS.length === 14, `expected the 14 subscribable events, got ${WEBHOOK_EVENTS.length}`);
});

suite.check('every event belongs to the group its prefix names', () => {
	for (const entry of WEBHOOK_EVENT_GROUPS) {
		for (const event of entry.events) {
			assert(event.startsWith(`${entry.group}.`), `${event} is filed under ${entry.group}`);
		}
	}
});

for (const locale of LOCALES) {
	suite.check(`${locale}: every event and group has a label`, () => {
		const messages = JSON.parse(readFileSync(join(FRONTEND, `src/lib/i18n/${locale}.json`), 'utf8'));
		const keys = [
			...WEBHOOK_EVENTS.map(webhookEventLabelKey),
			...WEBHOOK_EVENT_GROUPS.map((entry) => webhookEventGroupKey(entry.group))
		];
		const missing = keys.filter((key) => {
			const value = lookup(messages, key);
			return typeof value !== 'string' || value.trim() === '';
		});
		assertDeepEqual(missing, [], `${locale}.json is missing webhook event copy`);
	});
}

suite.check('the webhook page renders its checkboxes from the shared constant', () => {
	const page = readFileSync(PAGE, 'utf8');
	assert(/\{#each WEBHOOK_EVENT_GROUPS as entry/.test(page), 'the page does not iterate WEBHOOK_EVENT_GROUPS');
	assert(!/eventOptions\s*=/.test(page), 'the page still declares its own event list');
});

export const result = suite.result();

if (import.meta.main) finish(result);
