/**
 * Typed client for the authorization server's first-party endpoints (contract 11.3, 11.4, 11.8 to
 * 11.10): the consent page's two calls and the three levels of "Connected applications".
 *
 * These endpoints answer failures with a real HTTP status and a body that carries the same
 * `code`, `message` and `error_code` fields as the API envelope, so `apiClient` already turns
 * them into an `ApiResult` whose `code` is the status. Every response is validated here into a
 * typed value (no assertions): a response of an unexpected shape is a failure, never a guess.
 * Failure messages are English prose from the server and are never shown; the pages map
 * `errorCode` to translated text (`failureKey`).
 */
import { apiClient, type ApiResult } from './client';
import type { AuthorizeParams } from '../oauth/consent-request';

// ---------------------------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------------------------

type Json = Record<string, unknown>;

function isRecord(value: unknown): value is Json {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function text(value: unknown): string | null {
	return typeof value === 'string' ? value : null;
}
function textOrNull(value: unknown): string | null {
	return typeof value === 'string' ? value : null;
}
function count(value: unknown): number | null {
	return typeof value === 'number' && Number.isInteger(value) && value >= 0 ? value : null;
}
function texts(value: unknown): string[] | null {
	if (!Array.isArray(value)) return null;
	const out: string[] = [];
	for (const item of value) {
		if (typeof item !== 'string') return null;
		out.push(item);
	}
	return out;
}

// ---------------------------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------------------------

export interface OAuthFailure {
	/** HTTP status the server answered with (the envelope `code`), or 500 for a network error. */
	status: number;
	/** Machine-readable `error_code`; '' when the server sent none. */
	errorCode: string;
	/** Structured details (`too_many_grants` lists the connections). */
	details: unknown;
}

export type OAuthResult<T> = { ok: true; data: T } | { ok: false; failure: OAuthFailure };

function failure(result: ApiResult<unknown>): OAuthResult<never> {
	return {
		ok: false,
		failure: {
			status: result.code === 0 ? 500 : result.code,
			errorCode: result.error_code ?? '',
			details: result.details
		}
	};
}

function malformed(): OAuthResult<never> {
	return { ok: false, failure: { status: 502, errorCode: 'malformed_response', details: null } };
}

function parsed<T>(result: ApiResult<unknown>, parse: (data: unknown) => T | null): OAuthResult<T> {
	if (result.code !== 0) return failure(result);
	const data = parse(result.data);
	return data === null ? malformed() : { ok: true, data };
}

/** Translation key of the message for a failure; the pages pass it to `$t`. */
export function failureKey(error: OAuthFailure): string {
	switch (error.errorCode) {
		case 'consent_token_invalid':
		case 'workspace_not_eligible':
		case 'invalid_selection':
		case 'client_not_active':
		case 'rate_limited':
		case 'csrf_origin_mismatch':
		case 'session_required':
		case 'not_found':
		case 'request_not_found':
		case 'already_decided':
		case 'too_many_grants':
		case 'forbidden':
			return `oauth.errors.${error.errorCode}`;
	}
	if (error.status === 429) return 'oauth.errors.rate_limited';
	if (error.status === 403) return 'oauth.errors.forbidden';
	if (error.status === 404) return 'oauth.errors.not_found';
	return 'oauth.errors.generic';
}

// ---------------------------------------------------------------------------------------------
// Authorization request (consent page)
// ---------------------------------------------------------------------------------------------

export interface ClientDisplay {
	clientId: string;
	displayName: string;
	redirectHost: string;
	description: string;
	deviceName: string | null;
}

export interface ConsentWorkspace {
	id: string;
	name: string;
	eligible: boolean;
	reason: string | null;
}

export interface ConsentScope {
	name: string;
	required: boolean;
}

export interface ConsentData {
	requestId: string;
	consentToken: string;
	client: ClientDisplay;
	account: { userId: string; name: string; email: string };
	workspaces: ConsentWorkspace[];
	preselectedWorkspaceId: string | null;
	scopes: ConsentScope[];
	resources: string[];
	activeGrantCount: number;
	maxActiveGrants: number;
}

export type ErrorPageReason = 'unknown_client' | 'redirect_uri_mismatch' | 'request_expired';

export type AuthorizationOutcome =
	| { kind: 'error_page'; error: ErrorPageReason }
	| { kind: 'redirect'; redirectTo: string }
	| { kind: 'login_required'; requestId: string; client: ClientDisplay }
	| { kind: 'consent'; consent: ConsentData };

function parseClient(value: unknown): ClientDisplay | null {
	if (!isRecord(value)) return null;
	const clientId = text(value.client_id);
	const displayName = text(value.display_name);
	if (clientId === null || displayName === null) return null;
	return {
		clientId,
		displayName,
		redirectHost: text(value.redirect_host) ?? '',
		description: text(value.description) ?? '',
		deviceName: textOrNull(value.device_name)
	};
}

function parseWorkspace(value: unknown): ConsentWorkspace | null {
	if (!isRecord(value)) return null;
	const id = text(value.id);
	const name = text(value.name);
	if (id === null || name === null || typeof value.eligible !== 'boolean') return null;
	return { id, name, eligible: value.eligible, reason: textOrNull(value.reason) };
}

function parseConsent(value: Json): ConsentData | null {
	const requestId = text(value.request_id);
	const consentToken = text(value.consent_token);
	const client = parseClient(value.client);
	const account = value.account;
	const workspaces = value.workspaces;
	const scopes = value.scopes;
	const resources = texts(value.resources);
	const active = count(value.active_grant_count);
	const max = count(value.max_active_grants);
	if (
		requestId === null ||
		consentToken === null ||
		client === null ||
		!isRecord(account) ||
		!Array.isArray(workspaces) ||
		!Array.isArray(scopes) ||
		resources === null ||
		active === null ||
		max === null
	)
		return null;
	const userId = text(account.user_id);
	const name = text(account.name);
	const email = text(account.email);
	if (userId === null || name === null || email === null) return null;
	const parsedWorkspaces: ConsentWorkspace[] = [];
	for (const item of workspaces) {
		const workspace = parseWorkspace(item);
		if (workspace === null) return null;
		parsedWorkspaces.push(workspace);
	}
	const parsedScopes: ConsentScope[] = [];
	for (const item of scopes) {
		if (!isRecord(item)) return null;
		const scopeName = text(item.name);
		if (scopeName === null || typeof item.required !== 'boolean') return null;
		parsedScopes.push({ name: scopeName, required: item.required });
	}
	return {
		requestId,
		consentToken,
		client,
		account: { userId, name, email },
		workspaces: parsedWorkspaces,
		preselectedWorkspaceId: textOrNull(value.preselected_workspace_id),
		scopes: parsedScopes,
		resources,
		activeGrantCount: active,
		maxActiveGrants: max
	};
}

function parseOutcome(data: unknown): AuthorizationOutcome | null {
	if (!isRecord(data)) return null;
	switch (data.kind) {
		case 'error_page': {
			const reason = data.error;
			if (reason === 'redirect_uri_mismatch' || reason === 'request_expired')
				return { kind: 'error_page', error: reason };
			return { kind: 'error_page', error: 'unknown_client' };
		}
		case 'redirect': {
			const redirectTo = text(data.redirect_to);
			return redirectTo === null ? null : { kind: 'redirect', redirectTo };
		}
		case 'login_required': {
			const requestId = text(data.request_id);
			const client = parseClient(data.client);
			return requestId === null || client === null
				? null
				: { kind: 'login_required', requestId, client };
		}
		case 'consent': {
			const consent = parseConsent(data);
			return consent === null ? null : { kind: 'consent', consent };
		}
		default:
			return null;
	}
}

/** A request body: the parameters of a fresh request, or the id of a stored one. */
export type AuthorizationRequestInput =
	| { kind: 'new'; params: AuthorizeParams }
	| { kind: 'reload'; requestId: string };

export const AUTHORIZATION_REQUESTS_PATH = '/api/v1/oauth/authorization-requests';

export async function createAuthorizationRequest(
	input: AuthorizationRequestInput
): Promise<OAuthResult<AuthorizationOutcome>> {
	const body = input.kind === 'reload' ? { request_id: input.requestId } : input.params;
	const result = await apiClient.post<unknown>(AUTHORIZATION_REQUESTS_PATH, body);
	return parsed(result, parseOutcome);
}

// ---------------------------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------------------------

export type Decision =
	| {
			decision: 'allow';
			workspace_id: string;
			project_mode: 'all' | 'selected';
			project_ids?: string[];
			scopes: string[];
	  }
	| { decision: 'deny' }
	| { decision: 'no_account' };

export interface TooManyGrants {
	maxActiveGrants: number;
	grants: Array<{
		grantId: string;
		deviceName: string | null;
		workspaceId: string;
		consentedAt: string;
	}>;
}

/** The connections listed in a `too_many_grants` failure; null when `details` is not that shape. */
export function parseTooManyGrants(details: unknown): TooManyGrants | null {
	if (!isRecord(details)) return null;
	const max = count(details.max_active_grants);
	if (max === null || !Array.isArray(details.grants)) return null;
	const grants: TooManyGrants['grants'] = [];
	for (const item of details.grants) {
		if (!isRecord(item)) return null;
		const grantId = text(item.grant_id);
		const workspaceId = text(item.workspace_id);
		const consentedAt = text(item.consented_at);
		if (grantId === null || workspaceId === null || consentedAt === null) return null;
		grants.push({ grantId, deviceName: textOrNull(item.device_name), workspaceId, consentedAt });
	}
	return { maxActiveGrants: max, grants };
}

/**
 * Sends the decision. `consentToken` goes in the `X-Sylvode-Consent` header (single use), never
 * in the body or the URL. `no_account` needs no token and no session.
 */
export async function submitDecision(
	requestId: string,
	decision: Decision,
	consentToken: string | null
): Promise<OAuthResult<{ redirectTo: string }>> {
	const headers: Record<string, string> = {};
	if (consentToken !== null) headers['X-Sylvode-Consent'] = consentToken;
	const result = await apiClient.request<unknown>(
		'POST',
		`${AUTHORIZATION_REQUESTS_PATH}/${encodeURIComponent(requestId)}/decision`,
		decision,
		true,
		headers
	);
	return parsed(result, (data) => {
		if (!isRecord(data)) return null;
		const redirectTo = text(data.redirect_to);
		return redirectTo === null ? null : { redirectTo };
	});
}

// ---------------------------------------------------------------------------------------------
// Connected applications (management)
// ---------------------------------------------------------------------------------------------

export interface GrantProject {
	id: string;
	key: string;
	name: string;
}

export interface ConnectedGrant {
	grantId: string;
	client: { clientId: string; displayName: string };
	deviceName: string | null;
	workspace: { id: string; name: string };
	scopes: string[];
	projectMode: 'all' | 'selected';
	projects: GrantProject[];
	/** Number of projects of a `selected` grant (the list can be shorter); null for `all`. */
	projectCount: number | null;
	consentedAt: string;
	lastUsedAt: string | null;
	expiresAt: string;
	/** The member, on the workspace listing only. */
	user: { id: string; name: string; email: string } | null;
}

export interface GrantPage {
	items: ConnectedGrant[];
	hasMore: boolean;
	nextOffset: number | null;
}

function parseGrant(value: unknown): ConnectedGrant | null {
	if (!isRecord(value)) return null;
	const grantId = text(value.grant_id);
	const client = value.client;
	const workspace = value.workspace;
	const scopes = texts(value.scopes);
	const consentedAt = text(value.consented_at);
	const expiresAt = text(value.expires_at);
	const mode = value.project_mode;
	if (
		grantId === null ||
		!isRecord(client) ||
		!isRecord(workspace) ||
		scopes === null ||
		consentedAt === null ||
		expiresAt === null ||
		(mode !== 'all' && mode !== 'selected') ||
		!Array.isArray(value.projects)
	)
		return null;
	const clientId = text(client.client_id);
	const displayName = text(client.display_name);
	const workspaceId = text(workspace.id);
	const workspaceName = text(workspace.name);
	if (clientId === null || displayName === null || workspaceId === null || workspaceName === null)
		return null;
	const projects: GrantProject[] = [];
	for (const item of value.projects) {
		if (!isRecord(item)) return null;
		const id = text(item.id);
		const key = text(item.key);
		const name = text(item.name);
		if (id === null || key === null || name === null) return null;
		projects.push({ id, key, name });
	}
	let user: ConnectedGrant['user'] = null;
	if (value.user !== undefined) {
		const raw = value.user;
		if (!isRecord(raw)) return null;
		const id = text(raw.id);
		const name = text(raw.name);
		const email = text(raw.email);
		if (id === null || name === null || email === null) return null;
		user = { id, name, email };
	}
	return {
		grantId,
		client: { clientId, displayName },
		deviceName: textOrNull(value.device_name),
		workspace: { id: workspaceId, name: workspaceName },
		scopes,
		projectMode: mode,
		projects,
		projectCount: count(value.project_count),
		consentedAt,
		lastUsedAt: textOrNull(value.last_used_at),
		expiresAt,
		user
	};
}

function parseGrantPage(data: unknown): GrantPage | null {
	if (!isRecord(data) || !Array.isArray(data.items) || typeof data.has_more !== 'boolean')
		return null;
	const items: ConnectedGrant[] = [];
	for (const item of data.items) {
		const grant = parseGrant(item);
		if (grant === null) return null;
		items.push(grant);
	}
	return { items, hasMore: data.has_more, nextOffset: count(data.next_offset) };
}

const PAGE_LIMIT = 100;
/** A hard stop so a misbehaving server cannot keep the loop going. */
const MAX_PAGES = 100;

async function listAll(
	endpoint: (offset: number) => string
): Promise<OAuthResult<ConnectedGrant[]>> {
	const items: ConnectedGrant[] = [];
	let offset = 0;
	for (let page = 0; page < MAX_PAGES; page += 1) {
		const result = parsed(await apiClient.get<unknown>(endpoint(offset)), parseGrantPage);
		if (!result.ok) return result;
		items.push(...result.data.items);
		if (!result.data.hasMore || result.data.nextOffset === null) return { ok: true, data: items };
		offset = result.data.nextOffset;
	}
	return { ok: true, data: items };
}

function parseRevoked(data: unknown): { grantId: string } | null {
	if (!isRecord(data) || data.revoked !== true) return null;
	const grantId = text(data.grant_id);
	return grantId === null ? null : { grantId };
}

const enc = encodeURIComponent;

export const oauthApi = {
	/** Every active connection of the signed-in person, newest first. */
	listOwn(): Promise<OAuthResult<ConnectedGrant[]>> {
		return listAll((offset) => `/api/v1/me/connected-apps?limit=${PAGE_LIMIT}&offset=${offset}`);
	},

	async disconnectOwn(grantId: string): Promise<OAuthResult<{ grantId: string }>> {
		return parsed(
			await apiClient.delete<unknown>(`/api/v1/me/connected-apps/${enc(grantId)}`),
			parseRevoked
		);
	},

	/** Every active connection that reaches the workspace (owner or admin). */
	listWorkspace(workspaceId: string): Promise<OAuthResult<ConnectedGrant[]>> {
		return listAll(
			(offset) =>
				`/api/v1/workspaces/${enc(workspaceId)}/connected-apps?limit=${PAGE_LIMIT}&offset=${offset}`
		);
	},

	async disconnectInWorkspace(
		workspaceId: string,
		grantId: string
	): Promise<OAuthResult<{ grantId: string }>> {
		return parsed(
			await apiClient.delete<unknown>(
				`/api/v1/workspaces/${enc(workspaceId)}/connected-apps/${enc(grantId)}`
			),
			parseRevoked
		);
	},

	async getWorkspaceSetting(workspaceId: string): Promise<OAuthResult<{ allow: boolean }>> {
		return parsed(
			await apiClient.get<unknown>(
				`/api/v1/workspaces/${enc(workspaceId)}/connected-apps/settings`
			),
			(data) =>
				isRecord(data) && typeof data.allow_connected_apps === 'boolean'
					? { allow: data.allow_connected_apps }
					: null
		);
	},

	/** Turning it off revokes every active connection of the workspace; the count comes back. */
	async setWorkspaceSetting(
		workspaceId: string,
		allow: boolean
	): Promise<OAuthResult<{ allow: boolean; revokedGrants: number }>> {
		return parsed(
			await apiClient.put<unknown>(
				`/api/v1/workspaces/${enc(workspaceId)}/connected-apps/settings`,
				{ allow_connected_apps: allow }
			),
			(data) => {
				if (!isRecord(data) || typeof data.allow_connected_apps !== 'boolean') return null;
				const revoked = count(data.revoked_grants);
				return revoked === null
					? null
					: { allow: data.allow_connected_apps, revokedGrants: revoked };
			}
		);
	},

	async listClients(): Promise<OAuthResult<OAuthClient[]>> {
		return parsed(await apiClient.get<unknown>('/api/v1/admin/oauth-clients'), (data) => {
			if (!isRecord(data) || !Array.isArray(data.items)) return null;
			const items: OAuthClient[] = [];
			for (const item of data.items) {
				if (!isRecord(item)) return null;
				const clientId = text(item.client_id);
				const displayName = text(item.display_name);
				const active = count(item.active_grants);
				const status = item.status;
				if (
					clientId === null ||
					displayName === null ||
					active === null ||
					(status !== 'active' && status !== 'disabled')
				)
					return null;
				items.push({ clientId, displayName, status, activeGrants: active });
			}
			return items;
		});
	},

	/** Disabling revokes every active connection of the client; the count comes back. */
	async setClientStatus(
		clientId: string,
		status: 'active' | 'disabled'
	): Promise<OAuthResult<{ status: 'active' | 'disabled'; revokedGrants: number }>> {
		return parsed(
			await apiClient.patch<unknown>(`/api/v1/admin/oauth-clients/${enc(clientId)}`, { status }),
			(data) => {
				if (!isRecord(data)) return null;
				const next = data.status;
				const revoked = count(data.revoked_grants);
				if ((next !== 'active' && next !== 'disabled') || revoked === null) return null;
				return { status: next, revokedGrants: revoked };
			}
		);
	}
};

export interface OAuthClient {
	clientId: string;
	displayName: string;
	status: 'active' | 'disabled';
	activeGrants: number;
}
