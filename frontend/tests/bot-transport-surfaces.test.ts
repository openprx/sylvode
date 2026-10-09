/**
 * The bot token form offers exactly the transport surfaces the API binds credentials to, and
 * every one of them is explained in every shipped locale.
 *
 * A bot credential is accepted on one surface only (`apps/api/src/middleware/bot_auth.rs`,
 * `credential_bound_surface`), so a form that cannot choose one issues tokens that are refused
 * everywhere but the REST API. The server's single allow-list,
 * `EventSurface::BOT_CREDENTIAL_SURFACES` in `apps/api/src/flow/event_origin.rs`, is read at run
 * time, spelled through `EventSurface::as_wire`, and compared with `BOT_TRANSPORT_SURFACES`, so
 * adding a surface on either side without the other fails here. The route that validates
 * `transport_surface` (`apps/api/src/routes/bot.rs`) is checked to still validate against that
 * list rather than a copy of its own.
 *
 * The page itself is checked for the three places the surface has to appear: the create form
 * (as a required choice), the bot list and the token-reveal dialog.
 *
 * Run standalone: `bun tests/bot-transport-surfaces.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from './support/harness';
import { withoutComments } from './support/source';
import { createBotRequest } from '../src/lib/bots/create-bot-request';
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

const EVENT_ORIGIN = 'apps/api/src/flow/event_origin.rs';
const BOT_ROUTE = 'apps/api/src/routes/bot.rs';

/** Rust source with line and block comments removed, so commented-out code is never matched. */
function rustCode(relative: string): string {
	return readFileSync(join(REPO, relative), 'utf8')
		.replace(/\/\*[\s\S]*?\*\//g, '')
		.replace(/\/\/[^\n]*/g, '');
}

/**
 * The surfaces `EventSurface::BOT_CREDENTIAL_SURFACES` lists, in source order, as wire strings.
 *
 * Every step fails loudly when the source no longer has the expected shape; there is no fallback
 * list, because a fallback would turn a drift into a pass.
 */
function serverSurfaces(): string[] {
	const code = rustCode(EVENT_ORIGIN);

	const enumBody = code.match(/pub enum EventSurface \{([^}]*)\}/);
	assert(enumBody, `${EVENT_ORIGIN} no longer defines pub enum EventSurface`);
	const variants = [...enumBody[1].matchAll(/\b([A-Z][A-Za-z0-9]*)\s*,?/g)].map((m) => m[1]);
	assert(variants.length > 0, `${EVENT_ORIGIN}: EventSurface has no variants this test can read`);

	const implStart = code.indexOf('impl EventSurface {');
	assert(implStart >= 0, `${EVENT_ORIGIN} no longer has an inherent impl EventSurface block`);
	const impl = code.slice(implStart);

	const asWire = impl.match(/fn as_wire\(self\)[^{]*\{\s*match self \{([^}]*)\}/);
	assert(asWire, `${EVENT_ORIGIN}: EventSurface::as_wire is no longer a plain match on self`);
	const wire = new Map<string, string>();
	for (const arm of asWire[1].matchAll(/Self::([A-Za-z0-9]+)\s*=>\s*"([a-z0-9_]+)"/g)) {
		wire.set(arm[1], arm[2]);
	}
	assertDeepEqual(
		[...wire.keys()].sort(),
		[...variants].sort(),
		`${EVENT_ORIGIN}: as_wire must spell every EventSurface variant exactly once`
	);

	const list = impl.match(
		/pub const BOT_CREDENTIAL_SURFACES:\s*\[Self;\s*(\d+)\]\s*=\s*\[([^\]]*)\];/
	);
	assert(list, `${EVENT_ORIGIN} no longer defines EventSurface::BOT_CREDENTIAL_SURFACES as [Self; N]`);
	const declared = Number(list[1]);
	const items = list[2]
		.split(',')
		.map((item) => item.trim())
		.filter((item) => item !== '');
	assert(
		items.length === declared,
		`${EVENT_ORIGIN}: BOT_CREDENTIAL_SURFACES declares ${declared} entries but this test read ${items.length}`
	);
	return items.map((item) => {
		const variant = item.match(/^Self::([A-Za-z0-9]+)$/);
		assert(variant, `${EVENT_ORIGIN}: BOT_CREDENTIAL_SURFACES entry "${item}" is not Self::<Variant>`);
		const spelled = wire.get(variant[1]);
		assert(spelled, `${EVENT_ORIGIN}: BOT_CREDENTIAL_SURFACES entry ${item} has no as_wire spelling`);
		return spelled;
	});
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
	assertDeepEqual(
		[...BOT_TRANSPORT_SURFACES],
		serverSurfaces(),
		`frontend vs EventSurface::BOT_CREDENTIAL_SURFACES in ${EVENT_ORIGIN}`
	);
});

suite.check('the API validates transport_surface against that one list, not a copy of its own', () => {
	const code = rustCode(BOT_ROUTE);
	const body = code.match(/fn normalize_transport_surface\([\s\S]*?\n\}/);
	assert(body, `${BOT_ROUTE} no longer defines normalize_transport_surface`);
	assert(
		/EventSurface::from_bot_credential_label\(/.test(body[0]),
		'normalize_transport_surface must accept exactly what EventSurface::from_bot_credential_label accepts'
	);
	assert(
		!/"(?:rest|mcp_http|mcp_sse|mcp_stdio|cli|cli_tools_call|web|worker|system)"\s*[,\]]/.test(body[0]),
		'normalize_transport_surface must not carry its own list of surfaces'
	);
	const parse = rustCode(EVENT_ORIGIN).match(/fn from_bot_credential_label\([\s\S]*?\n\s{4}\}/);
	assert(parse, `${EVENT_ORIGIN} no longer defines EventSurface::from_bot_credential_label`);
	assert(
		/Self::BOT_CREDENTIAL_SURFACES/.test(parse[0]),
		'from_bot_credential_label must parse against Self::BOT_CREDENTIAL_SURFACES'
	);
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

suite.check('the create request carries exactly the chosen surface, for every surface', () => {
	for (const surface of BOT_TRANSPORT_SURFACES) {
		const built = createBotRequest({
			name: '  ci bot  ',
			permissions: ['read', 'write'],
			surface,
			expiresAt: '2030-01-01T23:59:59.000Z'
		});
		assert(built.ok, `${surface}: a named token with a surface must build`);
		assertDeepEqual(
			built.request,
			{
				name: 'ci bot',
				permissions: ['read', 'write'],
				transport_surface: surface,
				expires_at: '2030-01-01T23:59:59.000Z'
			},
			`${surface}: request body`
		);
	}
});

suite.check('the create request is refused without a surface or a name, never defaulted', () => {
	const noSurface = createBotRequest({ name: 'ci bot', permissions: [], surface: '' });
	assert(!noSurface.ok && noSurface.errorKey === 'members.transportSurfaceRequired', 'no surface chosen');
	const noName = createBotRequest({ name: '   ', permissions: [], surface: 'cli' });
	assert(!noName.ok && noName.errorKey === 'members.tokenNameRequired', 'blank name');
});

suite.check('the members page sends the request createBotRequest builds from the chosen surface', () => {
	const code = withoutComments(readFileSync(MEMBERS_PAGE, 'utf8'));
	assert(
		/createBotRequest\(\{[\s\S]{0,200}surface: tokenSurface[\s\S]{0,200}\}\)/.test(code),
		'createToken must build its request from the selected tokenSurface'
	);
	assert(/botsApi\.create\(workspaceId, built\.request\)/.test(code), 'createToken must send the built request');
	assert(!/transport_surface\s*:/.test(code), 'the page must not set transport_surface itself');
	assert(/bind:group=\{tokenSurface\}/.test(code), 'the radio group must bind to tokenSurface');
});

suite.check('the members page asks for the surface, sends it, lists it and shows it on reveal', () => {
	const page = withoutComments(readFileSync(MEMBERS_PAGE, 'utf8'));
	assert(/\{#each BOT_TRANSPORT_SURFACES as surface/.test(page), 'create form does not list the surfaces');
	assert(
		/type="radio"[\s\S]{0,120}name="tokenSurface"[\s\S]{0,200}required/.test(page),
		'the surface choice must be a required, labelled radio group'
	);
	assert(/<legend[^>]*>[\s\S]{0,80}members\.transportSurface'/.test(page), 'the radio group needs a legend');
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
