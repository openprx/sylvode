// The twenty cases of the contract's WP8 table, in order.

export interface CaseInfo {
	id: string;
	title: string;
}

export const CATALOG: CaseInfo[] = [
	{
		id: 'A01',
		title:
			'A connects with the reference scopes, projects {P1}: tokens for both resources, one row in Connected applications'
	},
	{
		id: 'A02',
		title: 'tools/list with the MCP token shows only the read tools of the granted scopes'
	},
	{
		id: 'A03',
		title: 'work_items.get on an item in P1 succeeds; in P2 it is refused and nothing changes'
	},
	{
		id: 'A04',
		title:
			'a write attempt is refused (MCP tool withheld, API route needs the write scope, consent cannot ask for it); item unchanged'
	},
	{ id: 'A05', title: 'the MCP token presented to the API is refused with 401 invalid_token' },
	{
		id: 'A06',
		title: 'the API token presented to the MCP server is refused with 401 invalid_token'
	},
	{
		id: 'A07',
		title: 'last hop, notification: B assigns an item in P1 to A, the event arrives within 5 s'
	},
	{
		id: 'A08',
		title:
			'last hop, task: task.created then task.status_changed (pending to processing), each within 5 s'
	},
	{ id: 'A09', title: 'the same as A07 with an item in P2: no event within 10 s' },
	{ id: 'A10', title: 'resume with Last-Event-ID returns exactly the two missed events in order' },
	{
		id: 'A11',
		title:
			'revocation from Sylvode (personal settings): revoked within 5 s, then nothing, reconnect and MCP answer 401'
	},
	{ id: 'A12', title: 'revocation from the client (refresh token): reason client_revoked' },
	{ id: 'A13', title: 'revocation by the workspace admin: reason workspace_admin_revoked' },
	{ id: 'A14', title: 'password change: reason password_changed, refresh answers invalid_grant' },
	{ id: 'A15', title: 'instance admin disables the account: reason account_disabled' },
	{
		id: 'A16',
		title: 'a refresh token used twice: invalid_grant, grant revoked with refresh_reuse'
	},
	{
		id: 'A17',
		title: 'an authorization code redeemed twice: invalid_grant, grant revoked with code_replay'
	},
	{
		id: 'A18',
		title:
			'deny and "I do not have an account" reach the loopback as access_denied; nothing is created'
	},
	{ id: 'A19', title: 'a localhost redirect URI shows the error page and never redirects' },
	{
		id: 'A20',
		title: 'the existing bot-token MCP regression script and the JWT UI smoke test still pass'
	}
];
