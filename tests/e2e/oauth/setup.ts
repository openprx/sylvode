// Creates what the suite needs, through the API, as the instance administrator. Idempotent.
//
//   node tests/e2e/oauth/setup.ts
//
// Creates or finds: users A and B, the workspace (name starts with "qa-p4-"), the members (A member,
// B admin), projects P1 and P2, the bot, and one work item in each project. The bot is made by
// connecting P1 (`bot.name`, which also creates its seat, token and webhook), uses the new token
// once (the worker holds a new bot's tasks until it has been used) and joins P2 by id:
// a bot is a project's AI participant only through a connection. The runner address must pass the
// API's outbound policy, so the suite's `sylvode.toml` lists it under `[outbound] allowed_hosts`
// (the `[outbound]` section). Refuses a non-loopback host unless OAUTH_E2E_ALLOW_NON_LOOPBACK=1. Prints
// ids only; never a password or a token.

import { loadConfig, NAME_PREFIX, type Account, type Config, ConfigError } from './config.ts';
import { call } from './lib/http.ts';
import { redactor } from './lib/redact.ts';
import { Recorder } from './lib/record.ts';
import { envelopeData, FirstParty, itemsOf } from './lib/sv.ts';
import { asRecord, check, errorMessage, str } from './lib/util.ts';

async function ensureUser(
	admin: FirstParty,
	rec: Recorder,
	body: {
		name: string;
		email: string;
		password?: string;
		entity_type?: string;
		agent_type?: string;
	}
): Promise<string> {
	const found = itemsOf(
		await admin.data(
			rec,
			`find ${body.email}`,
			'GET',
			`/api/v1/admin/users?search=${encodeURIComponent(body.email)}&limit=20`
		)
	).map(asRecord);
	const existing = found.find((u) => str(u?.email)?.toLowerCase() === body.email.toLowerCase());
	const existingId = str(existing?.id);
	if (existingId !== undefined) {
		log(`user ${body.email}: exists`);
		return existingId;
	}
	const created = asRecord(
		await admin.data(rec, `create ${body.email}`, 'POST', '/api/v1/admin/users', {
			role: 'user',
			...body
		})
	);
	const id = str(created?.id);
	check(id !== undefined, `creating ${body.email} returned no id`);
	log(`user ${body.email}: created`);
	return id;
}

function log(line: string): void {
	process.stdout.write(`${line}\n`);
}

async function ensureLogin(
	cfg: Config,
	role: string,
	account: Account,
	rec: Recorder
): Promise<void> {
	const res = await new FirstParty(cfg, role, account).loginWith(rec, account.password);
	check(
		res.status === 200 && asRecord(res.json)?.code === 0,
		`${role} cannot sign in with the configured password (HTTP ${res.status}); the account exists with another password`
	);
}

/** The token a connect answer carries inside its configuration block. */
const TOKEN_IN_BLOCK = /token = "(opr_[0-9a-f]{64})"/;

async function connectProject(
	admin: FirstParty,
	rec: Recorder,
	cfg: Config,
	label: string,
	projectId: string,
	bot: { name: string } | { id: string }
): Promise<string> {
	const path = `/api/v1/projects/${projectId}/bot-connections`;
	const read = asRecord(await admin.data(rec, `read the connections of ${label}`, 'GET', path));
	const listed = Array.isArray(read?.connections) ? read.connections : [];
	const existing = listed
		.map((entry) => asRecord(asRecord(asRecord(entry)?.connection)?.bot))
		.find((found) => ('id' in bot ? str(found?.id) === bot.id : str(found?.name) === bot.name));
	const existingId = str(existing?.id);
	if (existingId !== undefined) {
		log(`bot connection of ${label}: exists`);
		return existingId;
	}
	const answered = asRecord(
		await admin.data(rec, `connect the bot to ${label}`, 'POST', path, {
			bot,
			runner_url: cfg.runnerUrl,
			cli: 'claude',
			working_directory: cfg.workingDirectory
		})
	);
	const id = str(asRecord(asRecord(answered?.connection)?.bot)?.id);
	check(id !== undefined, `connecting ${label} returned no bot id`);
	const text = str(asRecord(answered?.config_block)?.text);
	if ('name' in bot) {
		// A new bot comes with its token in the block. It is shown once; the suite never needs it
		// (A20 uses its own transport-bound token), so it is only kept out of every record.
		const token = text === undefined ? undefined : TOKEN_IN_BLOCK.exec(text)?.[1];
		check(token !== undefined, 'the connect answer carries no configuration block with a token');
		redactor.register(token, 'bot-token:connection');
		// What a runner does when it starts: one authenticated call. Until the new token has been
		// used the worker holds the bot's tasks back (for at most an hour), so the suite's tasks
		// would wait; the call also proves the block's token works on the surface it was issued for.
		const handshake = await call({
			rec,
			label: 'use the new bot token once (the runner handshake)',
			method: 'GET',
			url: `${cfg.apiOrigin}/api/v1/ai/protocol`,
			bearer: token,
			headers: { 'x-sylvode-mcp-surface': 'mcp_stdio' }
		});
		check(
			handshake.status === 200 && asRecord(handshake.json)?.code === 0,
			`the block's token is refused by the protocol endpoint (HTTP ${handshake.status})`
		);
	}
	log(`bot connection of ${label}: created`);
	return id;
}

async function ensureBotConnections(
	admin: FirstParty,
	rec: Recorder,
	cfg: Config,
	ids: Record<string, string>
): Promise<string> {
	const p1 = ids.P1;
	const p2 = ids.P2;
	check(p1 !== undefined && p2 !== undefined, 'the projects are missing');
	const bot = await connectProject(admin, rec, cfg, 'P1', p1, { name: cfg.botName });
	const again = await connectProject(admin, rec, cfg, 'P2', p2, { id: bot });
	check(again === bot, 'P2 is connected to another bot than P1');
	return bot;
}

export async function runSetup(cfg: Config): Promise<void> {
	const rec = new Recorder();
	const admin = new FirstParty(cfg, 'admin', cfg.admin);
	redactor.register(cfg.userA.password, 'password:userA');
	redactor.register(cfg.userB.password, 'password:userB');
	await admin.login(rec); // fails clearly when the administrator account is wrong

	const userA = await ensureUser(admin, rec, {
		name: 'QA P4 user A',
		email: cfg.userA.email,
		password: cfg.userA.password
	});
	const userB = await ensureUser(admin, rec, {
		name: 'QA P4 user B',
		email: cfg.userB.email,
		password: cfg.userB.password
	});
	await ensureLogin(cfg, 'userA', cfg.userA, rec);
	await ensureLogin(cfg, 'userB', cfg.userB, rec);

	// Workspace, owned by the administrator who creates it.
	const workspaces = itemsOf(
		await admin.data(rec, 'list workspaces', 'GET', '/api/v1/workspaces')
	).map(asRecord);
	let workspaceId = str(workspaces.find((w) => str(w?.name) === cfg.workspaceName)?.id);
	if (workspaceId === undefined) {
		const created = asRecord(
			await admin.data(rec, 'create the workspace', 'POST', '/api/v1/workspaces', {
				name: cfg.workspaceName,
				slug: cfg.workspaceSlug
			})
		);
		workspaceId = str(created?.id);
		log(`workspace ${cfg.workspaceName}: created`);
	} else {
		log(`workspace ${cfg.workspaceName}: exists`);
	}
	check(workspaceId !== undefined, 'no workspace id');
	check(cfg.workspaceName.startsWith(NAME_PREFIX), 'the workspace name lost its prefix');

	// Members: A member, B admin.
	const members = itemsOf(
		await admin.data(
			rec,
			'list members',
			'GET',
			`/api/v1/workspaces/${workspaceId}/members?per_page=100`
		)
	).map(asRecord);
	for (const [who, id, role] of [
		['A', userA, 'member'],
		['B', userB, 'admin']
	] as const) {
		const current = str(members.find((m) => str(m?.user_id) === id)?.role);
		if (current === undefined) {
			await admin.data(rec, `add ${who}`, 'POST', `/api/v1/workspaces/${workspaceId}/members`, {
				user_id: id,
				role
			});
			log(`member ${who}: added as ${role}`);
		} else if (who === 'B' && current !== 'admin' && current !== 'owner') {
			await admin.data(
				rec,
				'make B an admin',
				'PATCH',
				`/api/v1/workspaces/${workspaceId}/members/${id}`,
				{ role: 'admin' }
			);
			log('member B: promoted to admin');
		} else {
			log(`member ${who}: ${current}`);
		}
	}

	// Projects and one work item each.
	const projects = itemsOf(
		await admin.data(
			rec,
			'list projects',
			'GET',
			`/api/v1/workspaces/${workspaceId}/projects?per_page=100`
		)
	).map(asRecord);
	const ids: Record<string, string> = {};
	for (const [label, key, name] of [
		['P1', cfg.p1Key, cfg.p1Name],
		['P2', cfg.p2Key, cfg.p2Name]
	] as const) {
		let id = str(projects.find((p) => str(p?.key) === key)?.id);
		if (id === undefined) {
			const created = asRecord(
				await admin.data(
					rec,
					`create ${label}`,
					'POST',
					`/api/v1/workspaces/${workspaceId}/projects`,
					{ key, name }
				)
			);
			id = str(created?.id);
			log(`project ${label} (${key}): created`);
		} else {
			log(`project ${label} (${key}): exists`);
		}
		check(id !== undefined, `no id for ${label}`);
		ids[label] = id;

		const listed = itemsOf(
			await admin.data(
				rec,
				`list items of ${label}`,
				'GET',
				`/api/v1/projects/${id}/issues?per_page=50&search=qa-p4-seed`
			)
		).map(asRecord);
		if (listed.some((item) => str(item?.title)?.startsWith('qa-p4-seed') === true)) {
			log(`work item in ${label}: exists`);
		} else {
			envelopeData(
				await admin.request(
					rec,
					`create the item of ${label}`,
					'POST',
					`/api/v1/projects/${id}/issues`,
					{ title: `qa-p4-seed-${label.toLowerCase()}` }
				),
				`create the item of ${label}`
			);
			log(`work item in ${label}: created`);
		}
	}

	// The bot: made by the connection of P1, added to P2 by id.
	const bot = await ensureBotConnections(admin, rec, cfg, ids);

	// Readiness hints.
	const as = await call({
		rec,
		label: 'authorization server metadata',
		method: 'GET',
		url: `${cfg.uiOrigin}/.well-known/oauth-authorization-server`
	});
	log(`authorization server metadata at the UI origin: HTTP ${as.status}`);
	log('');
	log('ids (not secrets):');
	log(`  workspace  ${workspaceId}`);
	log(`  P1         ${ids.P1}`);
	log(`  P2         ${ids.P2}`);
	log(`  user A     ${userA}`);
	log(`  user B     ${userB}`);
	log(`  bot        ${bot}`);
}

async function main(): Promise<void> {
	let cfg: Config;
	try {
		cfg = loadConfig(process.env, 'setup');
	} catch (error) {
		process.stderr.write(`${error instanceof ConfigError ? error.message : errorMessage(error)}\n`);
		process.exit(2);
	}
	try {
		await runSetup(cfg);
	} catch (error) {
		process.stderr.write(`setup failed: ${redactor.text(errorMessage(error))}\n`);
		process.exit(1);
	}
}

if (process.argv[1] !== undefined && import.meta.filename === process.argv[1]) await main();
