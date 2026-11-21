/**
 * What the consent page sends to `POST /api/v1/oauth/authorization-requests`, taken from the
 * address bar (contract 6.1 steps 2 and 3).
 *
 * The page forwards the parameters it knows and nothing else. A parameter that appears twice is
 * ambiguous, and the redirect URI is the one value the page must never guess: the request is
 * refused locally and nothing is sent. Nothing here is ever rendered; the server validates the
 * values and answers with the display data.
 */

import { isRequestId } from './return-to';

/** Parameters whose repeated use is how RFC 8707 names several resources. */
const REPEATABLE = new Set(['resource']);

const SINGLE_PARAMS = [
	'response_type',
	'client_id',
	'redirect_uri',
	'scope',
	'state',
	'code_challenge',
	'code_challenge_method',
	'device_name',
	'workspace_hint'
] as const;

/** A generous bound: a valid request is under 2 KiB. */
export const MAX_QUERY_LENGTH = 8192;

export interface AuthorizeParams {
	response_type?: string;
	client_id?: string;
	redirect_uri?: string;
	scope?: string;
	state?: string;
	code_challenge?: string;
	code_challenge_method?: string;
	device_name?: string;
	workspace_hint?: string;
	resource?: string[];
}

export type ParsedAuthorizeQuery =
	| { kind: 'reload'; requestId: string }
	| { kind: 'new'; params: AuthorizeParams }
	| { kind: 'expired' }
	| { kind: 'invalid' };

/** Decides what a visit to the consent route means. `search` is `location.search`. */
export function parseAuthorizeQuery(search: string): ParsedAuthorizeQuery {
	if (search.length > MAX_QUERY_LENGTH) return { kind: 'invalid' };
	const query = new URLSearchParams(search);

	if (query.has('request')) {
		const values = query.getAll('request');
		const [only] = values;
		if (values.length !== 1 || only === undefined) return { kind: 'invalid' };
		return isRequestId(only) ? { kind: 'reload', requestId: only } : { kind: 'expired' };
	}

	const params: AuthorizeParams = {};
	for (const name of SINGLE_PARAMS) {
		const values = query.getAll(name);
		if (values.length > 1 && !REPEATABLE.has(name)) return { kind: 'invalid' };
		const [value] = values;
		if (value !== undefined) params[name] = value;
	}
	const resources = query.getAll('resource');
	if (resources.length > 0) params.resource = resources;

	// Without a client and a return address there is nothing to validate against.
	if (!params.client_id || !params.redirect_uri) return { kind: 'invalid' };
	return { kind: 'new', params };
}
