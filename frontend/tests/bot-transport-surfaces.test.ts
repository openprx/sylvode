/**
 * The bot token form offers exactly the transport surfaces the API binds credentials to, and
 * every one of them is explained in every shipped locale.
 *
 * A bot credential is accepted on one surface only (`apps/api/src/middleware/bot_auth.rs`,
 * `credential_bound_surface`), so a form that cannot choose one issues tokens that are refused
 * everywhere but the REST API. The server's allow-list is read out of
 * `apps/api/src/routes/bot.rs` at run time and compared with `BOT_TRANSPORT_SURFACES`, so adding
 * a surface on either side without the other fails here.
 *
 * The page itself is checked for the three places the surface has to appear: the create form
 * (as a required choice), the bot list and the token-reveal dialog.
 *
 * Run standalone: `bun tests/bot-transport-surfaces.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from './support/harness';
import {
	BOT_TRANSPORT_SURFACES,
	isBotTransportSurface,
	surfaceDescriptionKey,
	surfaceLabelKey
} from '../src/lib/bots/transport-surfaces';

const FRONTEND = new URL('..', import.meta.url).pathname;
const REPO = join(FRONTEND, '..');
const LOCALES = ['zh', 'en'] as const;
const MEMBERS_PAGE = join(
	FRONTEND,
	'src/routes/(app)/workspace/[workspaceId]/members/+page.svelte'
);

const suite = new Suite('bot-transport-surfaces');

/** The surfaces `normalize_transport_surface` accepts, in source order. */
function serverSurfaces(): string[] {
	const source = readFileSync(join(REPO, 'apps/api/src/routes/bot.rs'), 'utf8');
	const body = source.match(/fn normalize_transport_surface[\s\S]*?\n\}/);
	assert(body, 'apps/api/src/routes/bot.rs no longer defines normalize_transport_surface');
	const list = body[0].match(/if \[([^\]]*)\]\.contains/);
	assert(list, 'normalize_transport_surface no longer checks an inline array of surfaces');
	return [...list[1].matchAll(/"([a-z_]+)"/g)].map((match) => match[1]);
}

function lookup(tree: unknown, key: string): unknown {
	return key.split('.').reduce<unknown>((node, part) => {
		if (node && typeof node === 'object' && part in node) {
			return (node as Record<string, unknown>)[part];
		}
		return undefined;
	}, tree);
}

suite.check('the form offers exactly the surfaces the API accepts, in the same order', () => {
	assertDeepEqual([...BOT_TRANSPORT_SURFACES], serverSurfaces(), 'frontend vs apps/api/src/routes/bot.rs');
});

suite.check('the API default for an omitted surface is still rest, which the form never relies on', () => {
	const source = readFileSync(join(REPO, 'apps/api/src/routes/bot.rs'), 'utf8');
	assert(
		/value\.unwrap_or_else\(\|\| "rest"\.to_string\(\)\)/.test(source),
		'normalize_transport_surface changed its default; re-check the README and this form'
	);
	const api = readFileSync(join(FRONTEND, 'src/lib/api/bots.ts'), 'utf8');
	assert(
		/transport_surface: BotTransportSurface;/.test(api),
		'CreateBotData.transport_surface must be required so every create names its surface'
	);
});

for (const locale of LOCALES) {
	suite.check(`${locale}: every surface has a non-empty label and description`, () => {
		const messages = JSON.parse(readFileSync(join(FRONTEND, `src/lib/i18n/${locale}.json`), 'utf8'));
		const missing: string[] = [];
		for (const surface of BOT_TRANSPORT_SURFACES) {
			for (const key of [surfaceLabelKey(surface), surfaceDescriptionKey(surface)]) {
				const value = lookup(messages, key);
				if (typeof value !== 'string' || value.trim() === '') missing.push(key);
			}
		}
		for (const key of [
			'members.transportSurface',
			'members.transportSurfaceHint',
			'members.transportSurfaceRequired',
			'members.transportSurfaceColumn',
			'members.tokenWorksOn',
			'members.tokenWorksOnlyThere'
		]) {
			const value = lookup(messages, key);
			if (typeof value !== 'string' || value.trim() === '') missing.push(key);
		}
		assertDeepEqual(missing, [], `${locale}.json is missing surface copy`);
		const reveal = lookup(messages, 'members.tokenWorksOn');
		assert(
			typeof reveal === 'string' && reveal.includes('{surface}'),
			`${locale}: members.tokenWorksOn must name the surface through {surface}`
		);
	});
}

suite.check('an unknown surface from a newer server is not mistaken for a known one', () => {
	assert(!isBotTransportSurface('web'), 'web is not a bot surface');
	assert(!isBotTransportSurface(''), 'the empty string is not a surface');
	for (const surface of BOT_TRANSPORT_SURFACES) assert(isBotTransportSurface(surface), surface);
});

suite.check('the members page asks for the surface, sends it, lists it and shows it on reveal', () => {
	const page = readFileSync(MEMBERS_PAGE, 'utf8');
	assert(/\{#each BOT_TRANSPORT_SURFACES as surface/.test(page), 'create form does not list the surfaces');
	assert(
		/type="radio"[\s\S]{0,120}name="tokenSurface"[\s\S]{0,200}required/.test(page),
		'the surface choice must be a required, labelled radio group'
	);
	assert(/<legend[^>]*>[\s\S]{0,80}members\.transportSurface'/.test(page), 'the radio group needs a legend');
	assert(/transport_surface: tokenSurface/.test(page), 'createToken does not send the chosen surface');
	assert(
		/let tokenSurface = \$state<BotTransportSurface \| ''>\(''\)/.test(page),
		'the surface must start unselected so the user has to choose'
	);
	assert(/getSurfaceLabel\(bot\.transport_surface\)/.test(page), 'the bot list does not show each surface');
	assert(
		/members\.tokenWorksOn'[\s\S]{0,120}createdToken\.transport_surface/.test(page),
		'the token-reveal dialog does not say where the token works'
	);
});

export const result = suite.result();

if (import.meta.main) finish(result);
