// Configuration, from environment variables only. See README.md for the meaning of each one.

export const NAME_PREFIX = 'qa-p4-';

export interface Account {
	email: string;
	password: string;
}

export interface Config {
	uiOrigin: string;
	apiOrigin: string;
	mcpOrigin: string;
	mcpResource: string;
	clientId: string;
	admin: Account;
	userA: Account;
	userB: Account;
	workspaceName: string;
	workspaceSlug: string;
	p1Key: string;
	p1Name: string;
	p2Key: string;
	p2Name: string;
	botEmail: string;
	botName: string;
	pinned: { workspaceId?: string; p1Id?: string; p2Id?: string; botId?: string };
	/** Output directory of the run; empty in modes that do not write results. */
	outDir: string;
	allowNonLoopback: boolean;
	oprBotToken: string | undefined;
	a20Override: string | undefined;
	repoRoot: string | undefined;
	buildCommit: string | undefined;
	limitMs: number;
	silenceMs: number;
	/** Consent flows the client starts per minute at most (the server limits decisions per user). */
	flowsPerMinute: number;
}

export type Env = Record<string, string | undefined>;

export class ConfigError extends Error {
	override name = 'ConfigError';
}

export type Mode = 'run' | 'setup';

function origin(name: string, value: string, problems: string[]): string {
	try {
		const url = new URL(value);
		if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('scheme');
		if (url.pathname !== '/' || url.search !== '' || url.hash !== '') throw new Error('path');
		return url.origin;
	} catch {
		problems.push(
			`${name} must be an origin such as http://127.0.0.1:18097 (scheme, host, port; no path)`
		);
		return value;
	}
}

export function isLoopbackHost(hostname: string): boolean {
	const bare = hostname.replace(/^\[|\]$/g, '');
	return bare === 'localhost' || bare === '::1' || /^127(?:\.\d{1,3}){3}$/.test(bare);
}

export function loadConfig(env: Env, mode: Mode): Config {
	const problems: string[] = [];
	const get = (name: string): string | undefined => {
		const value = env[name];
		return value === undefined || value === '' ? undefined : value;
	};
	const required = (name: string): string => {
		const value = get(name);
		if (value === undefined) {
			problems.push(`${name} is required`);
			return '';
		}
		return value;
	};

	const uiOrigin = origin(
		'OAUTH_E2E_UI_ORIGIN',
		get('OAUTH_E2E_UI_ORIGIN') ?? 'http://127.0.0.1:18097',
		problems
	);
	const apiOrigin = origin(
		'OAUTH_E2E_API_ORIGIN',
		get('OAUTH_E2E_API_ORIGIN') ?? 'http://127.0.0.1:18083',
		problems
	);
	const mcpOrigin = origin(
		'OAUTH_E2E_MCP_ORIGIN',
		get('OAUTH_E2E_MCP_ORIGIN') ?? 'http://127.0.0.1:8090',
		problems
	);
	const mcpResource = get('OAUTH_E2E_MCP_RESOURCE') ?? `${mcpOrigin}/mcp/rpc`;

	const allowNonLoopback = get('OAUTH_E2E_ALLOW_NON_LOOPBACK') === '1';
	if (!allowNonLoopback) {
		for (const [name, value] of [
			['OAUTH_E2E_UI_ORIGIN', uiOrigin],
			['OAUTH_E2E_API_ORIGIN', apiOrigin],
			['OAUTH_E2E_MCP_ORIGIN', mcpOrigin]
		] as const) {
			try {
				if (!isLoopbackHost(new URL(value).hostname)) {
					problems.push(
						`${name} is not a loopback host; this suite creates and changes data and refuses to run against ` +
							'anything else unless OAUTH_E2E_ALLOW_NON_LOOPBACK=1 is set'
					);
				}
			} catch {
				// already reported by origin()
			}
		}
	}

	const workspaceName = get('OAUTH_E2E_WORKSPACE_NAME') ?? `${NAME_PREFIX}acceptance`;
	const workspaceSlug =
		get('OAUTH_E2E_WORKSPACE_SLUG') ?? workspaceName.toLowerCase().replace(/[^a-z0-9-]+/g, '-');
	if (!workspaceName.startsWith(NAME_PREFIX)) {
		problems.push(
			`OAUTH_E2E_WORKSPACE_NAME must start with "${NAME_PREFIX}" (every mutation stays in that workspace)`
		);
	}
	if (!workspaceSlug.startsWith(NAME_PREFIX)) {
		problems.push(`OAUTH_E2E_WORKSPACE_SLUG must start with "${NAME_PREFIX}"`);
	}

	const outDir = get('OAUTH_E2E_OUT_DIR') ?? '';
	if (mode === 'run' && outDir === '')
		problems.push('OAUTH_E2E_OUT_DIR is required (the directory results are written to)');

	const config: Config = {
		uiOrigin,
		apiOrigin,
		mcpOrigin,
		mcpResource,
		clientId: get('OAUTH_E2E_CLIENT_ID') ?? 'ramflux',
		admin: {
			email: required('OAUTH_E2E_ADMIN_EMAIL'),
			password: required('OAUTH_E2E_ADMIN_PASSWORD')
		},
		userA: {
			email: required('OAUTH_E2E_USER_A_EMAIL'),
			password: required('OAUTH_E2E_USER_A_PASSWORD')
		},
		userB: {
			email: required('OAUTH_E2E_USER_B_EMAIL'),
			password: required('OAUTH_E2E_USER_B_PASSWORD')
		},
		workspaceName,
		workspaceSlug,
		p1Key: get('OAUTH_E2E_P1_KEY') ?? 'QPA',
		p1Name: get('OAUTH_E2E_P1_NAME') ?? `${NAME_PREFIX}alpha`,
		p2Key: get('OAUTH_E2E_P2_KEY') ?? 'QPB',
		p2Name: get('OAUTH_E2E_P2_NAME') ?? `${NAME_PREFIX}beta`,
		botEmail: get('OAUTH_E2E_BOT_EMAIL') ?? `${NAME_PREFIX}bot@e2e.sylvode.test`,
		botName: get('OAUTH_E2E_BOT_NAME') ?? `${NAME_PREFIX}bot`,
		pinned: {
			workspaceId: get('OAUTH_E2E_WORKSPACE_ID'),
			p1Id: get('OAUTH_E2E_P1_ID'),
			p2Id: get('OAUTH_E2E_P2_ID'),
			botId: get('OAUTH_E2E_BOT_ID')
		},
		outDir,
		allowNonLoopback,
		oprBotToken: get('OAUTH_E2E_OPR_BOT_TOKEN'),
		a20Override: get('OAUTH_E2E_A20_OVERRIDE'),
		repoRoot: get('OAUTH_E2E_REPO_ROOT'),
		buildCommit: get('OAUTH_E2E_BUILD_COMMIT'),
		limitMs: 5000,
		silenceMs: 10_000,
		flowsPerMinute: Number(get('OAUTH_E2E_FLOWS_PER_MINUTE') ?? '7')
	};

	for (const [name, a, b] of [
		['OAUTH_E2E_USER_A_EMAIL and OAUTH_E2E_USER_B_EMAIL', config.userA.email, config.userB.email],
		['OAUTH_E2E_USER_A_EMAIL and OAUTH_E2E_ADMIN_EMAIL', config.userA.email, config.admin.email],
		['OAUTH_E2E_USER_B_EMAIL and OAUTH_E2E_ADMIN_EMAIL', config.userB.email, config.admin.email]
	] as const) {
		if (a !== '' && a.toLowerCase() === b.toLowerCase()) problems.push(`${name} must differ`);
	}

	if (!Number.isInteger(config.flowsPerMinute) || config.flowsPerMinute < 1)
		problems.push('OAUTH_E2E_FLOWS_PER_MINUTE must be a positive integer');
	if (problems.length > 0) {
		throw new ConfigError(`configuration problems:\n- ${problems.join('\n- ')}`);
	}
	return config;
}

/** The values of every secret in the configuration, for the artifact self-check and redaction. */
export function secretsOf(config: Config): Array<{ label: string; value: string }> {
	const list: Array<{ label: string; value: string }> = [
		{ label: 'password:admin', value: config.admin.password },
		{ label: 'password:userA', value: config.userA.password },
		{ label: 'password:userB', value: config.userB.password }
	];
	if (config.oprBotToken !== undefined)
		list.push({ label: 'bot-token', value: config.oprBotToken });
	return list;
}
