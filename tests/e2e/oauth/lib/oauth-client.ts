// The client's half of the protocol, as Ramflux would run it: discovery, the authorization URL,
// the token endpoint (code exchange, refresh with rotation) and revocation. Secrets live in
// memory only and are registered with the redactor the moment they arrive.

import type { Config } from '../config.ts';
import { call, type HttpResult } from './http.ts';
import { redactor } from './redact.ts';
import type { Recorder } from './record.ts';
import { asArray, asRecord, check, num, str } from './util.ts';

export const REFERENCE_SCOPE =
	'projects:read work_items:read notifications:read tasks:read offline_access';

export type Resource = 'api' | 'mcp';

export interface Discovery {
	issuer: string;
	authorizationEndpoint: string;
	tokenEndpoint: string;
	revocationEndpoint: string;
	apiResource: string;
	mcpResource: string;
	metadata: Record<string, unknown>;
	apiPrm: Record<string, unknown>;
	/** Problems found while discovering; the first case asserts there are none. */
	problems: string[];
}

export interface OAuthEnv {
	cfg: Config;
	disc: Discovery;
}

async function getJson(
	rec: Recorder,
	label: string,
	url: string
): Promise<{ res: HttpResult; body: Record<string, unknown> | null }> {
	const res = await call({ rec, label, method: 'GET', url });
	return { res, body: res.status === 200 ? asRecord(res.json) : null };
}

/** Discovery as a client does it (contract 6.1 step 1). Throws only when the server is unusable. */
export async function discover(cfg: Config, rec: Recorder): Promise<Discovery> {
	const problems: string[] = [];
	const as = await getJson(
		rec,
		'authorization server metadata',
		`${cfg.uiOrigin}/.well-known/oauth-authorization-server`
	);
	check(
		as.body !== null,
		`authorization server metadata: HTTP ${as.res.status} (is [oauth] issuer configured and the proxy routing the well-known path?)`
	);
	const metadata = as.body;
	const issuer = str(metadata.issuer);
	if (issuer !== cfg.uiOrigin) problems.push('metadata issuer is not byte for byte the UI origin');
	const methods = asArray(metadata.code_challenge_methods_supported) ?? [];
	if (!methods.includes('S256')) problems.push('metadata does not list S256');
	if (metadata.authorization_response_iss_parameter_supported !== true) {
		problems.push('metadata does not advertise authorization_response_iss_parameter_supported');
	}
	const endpoint = (key: string): string => {
		const value = str(metadata[key]);
		if (value === undefined) {
			problems.push(`metadata has no ${key}`);
			return '';
		}
		return value;
	};

	const api = await getJson(
		rec,
		'API protected resource metadata',
		`${cfg.uiOrigin}/.well-known/oauth-protected-resource/api`
	);
	check(api.body !== null, `API protected resource metadata: HTTP ${api.res.status}`);
	const apiResource = str(api.body.resource);
	check(apiResource !== undefined, 'API protected resource metadata names no resource');
	if (!(asArray(api.body.authorization_servers) ?? []).includes(cfg.uiOrigin)) {
		problems.push(
			'API protected resource metadata does not name the issuer as its authorization server'
		);
	}

	try {
		const mcp = await getJson(
			rec,
			'MCP protected resource metadata',
			`${cfg.mcpOrigin}/.well-known/oauth-protected-resource`
		);
		if (mcp.body === null) problems.push(`MCP protected resource metadata: HTTP ${mcp.res.status}`);
		else if (str(mcp.body.resource) !== cfg.mcpResource) {
			problems.push(
				'MCP protected resource metadata names a resource other than OAUTH_E2E_MCP_RESOURCE'
			);
		}
	} catch {
		problems.push(
			'MCP protected resource metadata could not be fetched (is the MCP server running?)'
		);
	}

	return {
		issuer: issuer ?? '',
		authorizationEndpoint: endpoint('authorization_endpoint'),
		tokenEndpoint: endpoint('token_endpoint'),
		revocationEndpoint: endpoint('revocation_endpoint'),
		apiResource,
		mcpResource: cfg.mcpResource,
		metadata,
		apiPrm: api.body,
		problems
	};
}

export interface AuthorizeParams {
	redirectUri: string;
	scope: string;
	state: string;
	challenge: string;
	resources: string[];
	deviceName?: string;
	workspaceHint?: string;
}

export function buildAuthorizeUrl(env: OAuthEnv, p: AuthorizeParams): string {
	const url = new URL(env.disc.authorizationEndpoint);
	url.searchParams.set('response_type', 'code');
	url.searchParams.set('client_id', env.cfg.clientId);
	url.searchParams.set('redirect_uri', p.redirectUri);
	url.searchParams.set('scope', p.scope);
	url.searchParams.set('state', p.state);
	url.searchParams.set('code_challenge', p.challenge);
	url.searchParams.set('code_challenge_method', 'S256');
	for (const resource of p.resources) url.searchParams.append('resource', resource);
	if (p.deviceName !== undefined) url.searchParams.set('device_name', p.deviceName);
	if (p.workspaceHint !== undefined) url.searchParams.set('workspace_hint', p.workspaceHint);
	return url.toString();
}

export interface TokenResponse {
	res: HttpResult;
	accessToken: string | undefined;
	refreshToken: string | undefined;
	expiresIn: number | undefined;
	scope: string | undefined;
	grantId: string | undefined;
	workspaceId: string | undefined;
	tokenType: string | undefined;
	error: string | undefined;
}

function parseTokenResponse(res: HttpResult): TokenResponse {
	const body = asRecord(res.json);
	const accessToken = str(body?.access_token);
	const refreshToken = str(body?.refresh_token);
	redactor.register(accessToken, 'access');
	redactor.register(refreshToken, 'refresh');
	return {
		res,
		accessToken,
		refreshToken,
		expiresIn: num(body?.expires_in),
		scope: str(body?.scope),
		grantId: str(body?.grant_id),
		workspaceId: str(body?.workspace_id),
		tokenType: str(body?.token_type),
		error: str(body?.error)
	};
}

export async function exchangeCode(
	env: OAuthEnv,
	rec: Recorder,
	p: { code: string; redirectUri: string; verifier: string; resource: string; label?: string }
): Promise<TokenResponse> {
	redactor.register(p.code, 'code');
	redactor.register(p.verifier, 'verifier');
	return parseTokenResponse(
		await call({
			rec,
			label: p.label ?? 'token: authorization_code',
			method: 'POST',
			url: env.disc.tokenEndpoint,
			form: {
				grant_type: 'authorization_code',
				code: p.code,
				redirect_uri: p.redirectUri,
				client_id: env.cfg.clientId,
				code_verifier: p.verifier,
				resource: p.resource
			}
		})
	);
}

export async function refreshGrant(
	env: OAuthEnv,
	rec: Recorder,
	p: { refreshToken: string; resource: string; scope?: string; label?: string }
): Promise<TokenResponse> {
	const form: Record<string, string> = {
		grant_type: 'refresh_token',
		refresh_token: p.refreshToken,
		client_id: env.cfg.clientId,
		resource: p.resource
	};
	if (p.scope !== undefined) form.scope = p.scope;
	return parseTokenResponse(
		await call({
			rec,
			label: p.label ?? 'token: refresh_token',
			method: 'POST',
			url: env.disc.tokenEndpoint,
			form
		})
	);
}

export function revokeToken(
	env: OAuthEnv,
	rec: Recorder,
	p: { token: string; hint: 'refresh_token' | 'access_token'; label?: string }
): Promise<HttpResult> {
	return call({
		rec,
		label: p.label ?? `revoke (${p.hint})`,
		method: 'POST',
		url: env.disc.revocationEndpoint,
		form: { token: p.token, token_type_hint: p.hint, client_id: env.cfg.clientId }
	});
}

interface Held {
	token: string;
	expiresAt: number;
}

/**
 * One connection (grant) as the client holds it: the current refresh token and the latest access
 * token per resource. Refreshing stores the new refresh token before it returns anything, as the
 * contract requires of a client (15.3), and is serialised so two callers never reuse a token.
 */
export class Connection {
	readonly grantId: string;
	readonly workspaceId: string;
	refreshToken: string;
	private held: Partial<Record<Resource, Held>> = {};
	private chain: Promise<unknown> = Promise.resolve();
	private readonly env: OAuthEnv;

	constructor(env: OAuthEnv, init: { grantId: string; workspaceId: string; refreshToken: string }) {
		this.env = env;
		this.grantId = init.grantId;
		this.workspaceId = init.workspaceId;
		this.refreshToken = init.refreshToken;
	}

	hold(resource: Resource, token: string, expiresInSeconds: number): void {
		this.held[resource] = { token, expiresAt: Date.now() + expiresInSeconds * 1000 };
	}

	resourceUrl(resource: Resource): string {
		return resource === 'api' ? this.env.disc.apiResource : this.env.disc.mcpResource;
	}

	/** Refreshes for `resource` with the current refresh token (rotation included). */
	rotate(rec: Recorder, resource: Resource, label?: string): Promise<TokenResponse> {
		const run = async (): Promise<TokenResponse> => {
			const response = await refreshGrant(this.env, rec, {
				refreshToken: this.refreshToken,
				resource: this.resourceUrl(resource),
				label
			});
			if (
				response.res.status === 200 &&
				response.accessToken !== undefined &&
				response.refreshToken !== undefined
			) {
				this.refreshToken = response.refreshToken;
				this.hold(resource, response.accessToken, response.expiresIn ?? 600);
			}
			return response;
		};
		const next = this.chain.then(run, run);
		this.chain = next.catch(() => undefined);
		return next;
	}

	/** An access token for `resource` with at least 45 seconds left; refreshes when needed. */
	async access(rec: Recorder, resource: Resource): Promise<string> {
		const current = this.held[resource];
		if (current !== undefined && current.expiresAt - Date.now() > 45_000) return current.token;
		const response = await this.rotate(rec, resource, `token: refresh for ${resource}`);
		check(
			response.accessToken !== undefined,
			`refresh for ${resource} failed with HTTP ${response.res.status} (${response.error ?? 'no error code'})`
		);
		return response.accessToken;
	}

	/** The token as last held, without refreshing (used to prove a dead token stays dead). */
	lastHeld(resource: Resource): string | undefined {
		return this.held[resource]?.token;
	}

	/** Revokes the grant the way the client does: the refresh token with the revocation endpoint. */
	revokeFromClient(rec: Recorder): Promise<HttpResult> {
		return revokeToken(this.env, rec, {
			token: this.refreshToken,
			hint: 'refresh_token',
			label: 'revoke from the client'
		});
	}
}
