// An in-process fake of the Sylvode endpoints the suite touches: first-party API, the OAuth
// authorization server, the delegated API with its event stream, and the MCP resource. It exists
// to prove the client's own logic (and its failure detection) before the real run; it is not a
// model of Sylvode. Faults can be switched on to show that the suite notices them.

import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { AUTHORIZE_PAGE, LOGIN_PAGE, SETTINGS_PAGE, workspaceSettingsPage } from './pages.ts';

export type Fault = 'none' | 'p2-event' | 'refresh-reuse' | 'no-revoked-control';

export interface FakeOptions {
	uiPort: number;
	mcpPort: number;
	fault: Fault;
	admin: { email: string; password: string };
}

interface User {
	id: string;
	email: string;
	name: string;
	password: string;
	role: 'admin' | 'user';
	active: boolean;
	entityType: 'human' | 'bot';
	sv: number;
}
interface Workspace {
	id: string;
	name: string;
	slug: string;
}
interface Member {
	workspaceId: string;
	userId: string;
	role: string;
}
interface Project {
	id: string;
	workspaceId: string;
	key: string;
	name: string;
}
interface Issue {
	id: string;
	projectId: string;
	title: string;
	assigneeId: string | null;
	version: number;
}
interface Task {
	id: string;
	projectId: string;
	botId: string;
	status: string;
	referenceId: string | null;
}
interface Grant {
	id: string;
	userId: string;
	workspaceId: string;
	projectMode: 'all' | 'selected';
	projectIds: string[];
	scopes: string[];
	status: 'active' | 'revoked';
	reason?: string;
	sv: number;
	deviceName: string;
}
interface Tok {
	kind: 'access' | 'refresh' | 'code';
	grantId: string;
	resource?: string;
	revoked: boolean;
	consumed: boolean;
	expiresAt: number;
	redirectUri?: string;
	challenge?: string;
	resources?: string[];
}
interface AuthRequest {
	id: string;
	params: Record<string, string>;
	resources: string[];
	scopes: string[];
	consentToken: string;
	decided: boolean;
}
interface FeedEvent {
	seq: number;
	type: string;
	workspaceId: string;
	projectId: string | null;
	userId: string | null;
	subject: Record<string, unknown>;
}
interface Stream {
	res: ServerResponse;
	grantId: string;
	tokenId: string;
	types: Set<string> | null;
	timer: NodeJS.Timeout;
}

const READ_SCOPES = ['projects:read', 'work_items:read', 'notifications:read', 'tasks:read'];
const ALLOWED_SCOPES = new Set([...READ_SCOPES, 'offline_access']);
const MCP_SCOPES = new Set([
	'projects:read',
	'work_items:read',
	'work_items:write',
	'forms:read',
	'forms:write'
]);
const READ_TOOLS = [
	'projects.list',
	'projects.get',
	'context.get_project',
	'work_items.list',
	'work_items.get',
	'work_items.get_by_identifier',
	'work_items.list_labels',
	'comments.list'
];
const sha = (text: string): string => createHash('sha256').update(text).digest('base64url');
const hex = (n: number): string => randomBytes(n).toString('hex');

export class Fake {
	readonly ui: string;
	readonly mcp: string;
	private readonly opts: FakeOptions;
	private servers: Server[] = [];
	readonly users = new Map<string, User>();
	private readonly sessions = new Map<string, { userId: string; sv: number }>();
	private readonly workspaces = new Map<string, Workspace>();
	private readonly members: Member[] = [];
	private readonly projects = new Map<string, Project>();
	private readonly issues = new Map<string, Issue>();
	private readonly participants = new Set<string>();
	private readonly tasks = new Map<string, Task>();
	private readonly grants = new Map<string, Grant>();
	private readonly tokens = new Map<string, Tok>();
	private readonly requests = new Map<string, AuthRequest>();
	private readonly feed: FeedEvent[] = [];
	private readonly streams = new Set<Stream>();
	private clientActive = true;
	private seq = 0;

	constructor(opts: FakeOptions) {
		this.opts = opts;
		this.ui = `http://127.0.0.1:${opts.uiPort}`;
		this.mcp = `http://127.0.0.1:${opts.mcpPort}`;
		this.addUser(opts.admin.email, 'Admin', opts.admin.password, 'admin');
	}

	get mcpResource(): string {
		return `${this.mcp}/mcp/rpc`;
	}

	async start(): Promise<void> {
		const ui = createServer(
			(req, res) => void this.handleUi(req, res).catch((error: unknown) => this.crash(res, error))
		);
		const mcp = createServer(
			(req, res) => void this.handleMcp(req, res).catch((error: unknown) => this.crash(res, error))
		);
		this.servers = [ui, mcp];
		await Promise.all([
			new Promise<void>((resolve) => ui.listen(this.opts.uiPort, '127.0.0.1', resolve)),
			new Promise<void>((resolve) => mcp.listen(this.opts.mcpPort, '127.0.0.1', resolve))
		]);
	}

	async stop(): Promise<void> {
		for (const stream of this.streams) {
			clearInterval(stream.timer);
			stream.res.end();
		}
		await Promise.all(
			this.servers.map(
				(server) =>
					new Promise<void>((resolve) => {
						server.closeAllConnections();
						server.close(() => resolve());
					})
			)
		);
	}

	private crash(res: ServerResponse, error: unknown): void {
		if (!res.headersSent) res.writeHead(500, { 'content-type': 'application/json' });
		res.end(
			JSON.stringify({
				error: 'fake_crashed',
				error_description: error instanceof Error ? error.message : 'error'
			})
		);
	}

	// ---- state helpers ------------------------------------------------------------------------

	private addUser(
		email: string,
		name: string,
		password: string,
		role: 'admin' | 'user',
		entityType: 'human' | 'bot' = 'human'
	): User {
		const user: User = {
			id: randomUUID(),
			email: email.toLowerCase(),
			name,
			password,
			role,
			active: true,
			entityType,
			sv: 1
		};
		this.users.set(user.id, user);
		return user;
	}

	private roleIn(workspaceId: string, userId: string): string | undefined {
		return this.members.find((m) => m.workspaceId === workspaceId && m.userId === userId)?.role;
	}

	private jwtFor(user: User): string {
		const token = `eyJhbGciOiJIUzI1NiJ9.${Buffer.from(JSON.stringify({ sub: user.id, n: hex(4) })).toString('base64url')}.${hex(16)}`;
		this.sessions.set(token, { userId: user.id, sv: user.sv });
		return token;
	}

	private sessionUser(req: IncomingMessage): User | null {
		const header = req.headers.authorization;
		let token = header?.startsWith('Bearer ') ? header.slice(7) : undefined;
		if (token === undefined) {
			const cookie = /(?:^|; )fake_session=([^;]+)/.exec(req.headers.cookie ?? '');
			token = cookie?.[1];
		}
		if (token === undefined || token.startsWith('sva_')) return null;
		const session = this.sessions.get(token);
		const user = session === undefined ? undefined : this.users.get(session.userId);
		if (user === undefined || session === undefined || !user.active || user.sv !== session.sv)
			return null;
		return user;
	}

	private revokeGrant(id: string, reason: string): void {
		const grant = this.grants.get(id);
		if (grant === undefined || grant.status === 'revoked') return;
		grant.status = 'revoked';
		grant.reason = reason;
		for (const token of this.tokens.values()) if (token.grantId === id) token.revoked = true;
		setTimeout(() => {
			for (const stream of [...this.streams]) {
				if (stream.grantId !== id) continue;
				if (!(this.opts.fault === 'no-revoked-control' && reason === 'user_revoked'))
					this.sendControl(stream, { type: 'revoked', reason });
				this.closeStream(stream);
			}
		}, 40);
	}

	private revokeAllOf(userId: string, reason: string): void {
		for (const grant of this.grants.values())
			if (grant.userId === userId) this.revokeGrant(grant.id, reason);
	}

	// ---- events -------------------------------------------------------------------------------

	private cursor(seq: number): string {
		return `cur-${seq}`;
	}

	private emit(
		type: string,
		workspaceId: string,
		projectId: string | null,
		userId: string | null,
		subject: Record<string, unknown>
	): void {
		this.seq += 1;
		const event: FeedEvent = { seq: this.seq, type, workspaceId, projectId, userId, subject };
		this.feed.push(event);
		setTimeout(() => {
			for (const stream of this.streams) this.deliver(stream, event);
		}, 60);
	}

	private visible(grant: Grant, event: FeedEvent): boolean {
		if (grant.status !== 'active' || event.workspaceId !== grant.workspaceId) return false;
		const notification = event.type.startsWith('notification.');
		if (!grant.scopes.includes(notification ? 'notifications:read' : 'tasks:read')) return false;
		if (notification && event.userId !== grant.userId) return false;
		if (
			grant.projectMode === 'selected' &&
			event.projectId !== null &&
			!grant.projectIds.includes(event.projectId)
		) {
			// Fault: the project filter is skipped for notifications.
			return this.opts.fault === 'p2-event' && notification;
		}
		return true;
	}

	private frame(event: FeedEvent): string {
		const project = event.projectId === null ? null : this.projects.get(event.projectId);
		const envelope = {
			spec: 'sylvode.delegated_event.v1',
			id: this.cursor(event.seq),
			type: event.type,
			occurred_at: new Date().toISOString(),
			workspace_id: event.workspaceId,
			project:
				project === null || project === undefined
					? null
					: { id: project.id, key: project.key, name: project.name },
			subject: event.subject,
			links: { web: `${this.ui}/workspace/${event.workspaceId}` }
		};
		return `id: ${this.cursor(event.seq)}\nevent: sv.event\ndata: ${JSON.stringify(envelope)}\n\n`;
	}

	private deliver(stream: Stream, event: FeedEvent): void {
		const grant = this.grants.get(stream.grantId);
		if (grant === undefined || !this.visible(grant, event)) return;
		if (stream.types !== null && !stream.types.has(event.type)) return;
		stream.res.write(this.frame(event));
	}

	private sendControl(stream: Stream, body: Record<string, unknown>): void {
		stream.res.write(`event: sv.control\ndata: ${JSON.stringify(body)}\n\n`);
	}

	private closeStream(stream: Stream): void {
		clearInterval(stream.timer);
		this.streams.delete(stream);
		stream.res.end();
	}

	// ---- HTTP plumbing ------------------------------------------------------------------------

	private async body(req: IncomingMessage): Promise<string> {
		const chunks: Buffer[] = [];
		for await (const chunk of req) chunks.push(chunk as Buffer);
		return Buffer.concat(chunks).toString('utf8');
	}

	private send(
		res: ServerResponse,
		status: number,
		body: unknown,
		headers: Record<string, string> = {}
	): void {
		res.writeHead(status, { 'content-type': 'application/json', ...headers });
		res.end(JSON.stringify(body));
	}

	private ok(res: ServerResponse, data: unknown, headers: Record<string, string> = {}): void {
		this.send(res, 200, { code: 0, message: 'success', data }, headers);
	}

	private deny(res: ServerResponse, code: number, message: string): void {
		this.send(res, 200, { code, message });
	}

	private html(res: ServerResponse, page: string): void {
		res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
		res.end(page);
	}

	private bearerFail(
		res: ServerResponse,
		status: number,
		error: string,
		description: string,
		scope?: string
	): void {
		const challenge = `Bearer realm="sylvode", error="${error}", error_description="${description}"${scope === undefined ? '' : `, scope="${scope}"`}`;
		this.send(
			res,
			status,
			{ code: status, message: description, error_code: error, error },
			{ 'www-authenticate': challenge, 'cache-control': 'no-store' }
		);
	}

	// ---- the UI origin (pages and API) -----------------------------------------------------------

	private async handleUi(req: IncomingMessage, res: ServerResponse): Promise<void> {
		const url = new URL(req.url ?? '/', this.ui);
		const path = url.pathname;
		const method = req.method ?? 'GET';
		if (method === 'GET') {
			if (path === '/oauth/authorize') return this.html(res, AUTHORIZE_PAGE);
			if (path === '/auth/login') return this.html(res, LOGIN_PAGE);
			if (path === '/settings') return this.html(res, SETTINGS_PAGE);
			const ws = /^\/workspace\/([^/]+)\/settings$/.exec(path);
			if (ws?.[1] !== undefined) return this.html(res, workspaceSettingsPage(ws[1]));
			if (path === '/workspace') return this.html(res, '<p>workspace</p>');
			if (path === '/.well-known/oauth-authorization-server') {
				return this.send(res, 200, {
					issuer: this.ui,
					authorization_endpoint: `${this.ui}/oauth/authorize`,
					token_endpoint: `${this.ui}/api/v1/oauth/token`,
					revocation_endpoint: `${this.ui}/api/v1/oauth/revoke`,
					code_challenge_methods_supported: ['S256'],
					authorization_response_iss_parameter_supported: true
				});
			}
			if (path === '/.well-known/oauth-protected-resource/api') {
				return this.send(res, 200, {
					resource: `${this.ui}/api`,
					authorization_servers: [this.ui]
				});
			}
			if (path === '/health') return this.ok(res, { status: 'ok', service: 'fake' });
		}
		if (path.startsWith('/api/v1/'))
			return this.handleApi(req, res, url, method, await this.body(req));
		res.writeHead(404);
		res.end();
	}

	private async handleApi(
		req: IncomingMessage,
		res: ServerResponse,
		url: URL,
		method: string,
		raw: string
	): Promise<void> {
		const path = url.pathname.slice('/api/v1'.length);
		let json: Record<string, unknown> = {};
		try {
			json =
				raw.trim() === '' || (req.headers['content-type'] ?? '').includes('urlencoded')
					? {}
					: (JSON.parse(raw) as Record<string, unknown>);
		} catch {
			json = {};
		}
		const form = new URLSearchParams(raw);

		// OAuth protocol endpoints (no first-party session needed).
		if (path === '/oauth/token' && method === 'POST') return this.token(res, form);
		if (path === '/oauth/revoke' && method === 'POST') return this.revoke(res, form);
		if (path === '/oauth/authorization-requests' && method === 'POST')
			return this.authorizationRequest(req, res, json);
		const decision = /^\/oauth\/authorization-requests\/([^/]+)\/decision$/.exec(path);
		if (decision?.[1] !== undefined && method === 'POST')
			return this.decision(req, res, decision[1], json);
		if (path === '/auth/login' && method === 'POST') return this.login(res, json);

		// Delegated bearer branch.
		const bearer = req.headers.authorization?.startsWith('Bearer ')
			? req.headers.authorization.slice(7)
			: '';
		if (bearer.startsWith('sva_')) return this.delegated(req, res, url, method, path, json, bearer);

		const user = this.sessionUser(req);
		if (user === null) return this.deny(res, 401, 'invalid access token');
		return this.firstParty(req, res, url, method, path, json, user);
	}

	private login(res: ServerResponse, body: Record<string, unknown>): void {
		const user = [...this.users.values()].find(
			(u) => u.email === String(body.email ?? '').toLowerCase()
		);
		if (user === undefined || !user.active || user.password !== body.password)
			return this.deny(res, 401, 'invalid credentials');
		const jwt = this.jwtFor(user);
		this.ok(
			res,
			{ tokens: { access_token: jwt }, user: { id: user.id, email: user.email } },
			{ 'set-cookie': `fake_session=${jwt}; Path=/; HttpOnly` }
		);
	}

	// ---- first-party API -----------------------------------------------------------------------

	private page<T>(items: T[]): { items: T[]; total: number; page: number; per_page: number } {
		return { items, total: items.length, page: 1, per_page: items.length };
	}

	private grantView(grant: Grant, withUser = false): Record<string, unknown> {
		const workspace = this.workspaces.get(grant.workspaceId);
		const user = this.users.get(grant.userId);
		return {
			grant_id: grant.id,
			client: { client_id: 'ramflux', display_name: 'Ramflux' },
			device_name: grant.deviceName,
			workspace: { id: grant.workspaceId, name: workspace?.name },
			scopes: grant.scopes,
			project_mode: grant.projectMode,
			...(withUser ? { user: { name: user?.name, email: user?.email } } : {})
		};
	}

	private issueView(issue: Issue): Record<string, unknown> {
		return {
			id: issue.id,
			project_id: issue.projectId,
			title: issue.title,
			state: 'todo',
			priority: 'medium',
			assignee_id: issue.assigneeId,
			updated_at: `v${issue.version}`
		};
	}

	private firstParty(
		req: IncomingMessage,
		res: ServerResponse,
		url: URL,
		method: string,
		path: string,
		body: Record<string, unknown>,
		user: User
	): void {
		const need = (admin: boolean): boolean => {
			if (admin && user.role !== 'admin') {
				this.deny(res, 403, 'admin required');
				return false;
			}
			return true;
		};
		if (path === '/auth/me/password' && method === 'PUT') {
			if (user.password !== body.current_password)
				return this.deny(res, 400, 'current password is wrong');
			user.password = String(body.new_password);
			user.sv += 1;
			this.revokeAllOf(user.id, 'password_changed');
			return this.ok(res, {});
		}
		if (path === '/admin/users' && method === 'GET') {
			if (!need(true)) return;
			const search = (url.searchParams.get('search') ?? '').toLowerCase();
			const list = [...this.users.values()].filter((u) => u.email.includes(search));
			return this.ok(res, {
				...this.page(list.map((u) => ({ id: u.id, email: u.email, name: u.name }))),
				total: list.length
			});
		}
		if (path === '/admin/users' && method === 'POST') {
			if (!need(true)) return;
			const email = String(body.email).toLowerCase();
			if ([...this.users.values()].some((u) => u.email === email))
				return this.deny(res, 409, 'email already exists');
			const created = this.addUser(
				email,
				String(body.name),
				String(body.password ?? ''),
				'user',
				body.entity_type === 'bot' ? 'bot' : 'human'
			);
			return this.ok(res, { id: created.id, email: created.email });
		}
		const status = /^\/admin\/users\/([^/]+)\/status$/.exec(path);
		if (status?.[1] !== undefined && method === 'PATCH') {
			if (!need(true)) return;
			const target = this.users.get(status[1]);
			if (target === undefined) return this.deny(res, 404, 'user not found');
			target.active = body.is_active === true;
			if (!target.active) {
				target.sv += 1;
				this.revokeAllOf(target.id, 'account_disabled');
			}
			return this.ok(res, {});
		}
		const reset = /^\/admin\/users\/([^/]+)\/password$/.exec(path);
		if (reset?.[1] !== undefined && method === 'PUT') {
			if (!need(true)) return;
			const target = this.users.get(reset[1]);
			if (target === undefined) return this.deny(res, 404, 'user not found');
			target.password = String(body.new_password);
			target.sv += 1;
			this.revokeAllOf(target.id, 'password_changed');
			return this.ok(res, {});
		}
		if (path === '/admin/oauth-clients' && method === 'GET') {
			if (!need(true)) return;
			return this.ok(
				res,
				this.page([{ client_id: 'ramflux', status: this.clientActive ? 'active' : 'disabled' }])
			);
		}
		if (path === '/admin/oauth-clients/ramflux' && method === 'PATCH') {
			if (!need(true)) return;
			this.clientActive = body.status === 'active';
			if (!this.clientActive)
				for (const g of this.grants.values()) this.revokeGrant(g.id, 'client_disabled');
			return this.ok(res, {});
		}
		if (path === '/workspaces' && method === 'GET') {
			const mine = [...this.workspaces.values()].filter(
				(w) => this.roleIn(w.id, user.id) !== undefined
			);
			return this.ok(res, this.page(mine));
		}
		if (path === '/workspaces' && method === 'POST') {
			const workspace: Workspace = {
				id: randomUUID(),
				name: String(body.name),
				slug: String(body.slug)
			};
			this.workspaces.set(workspace.id, workspace);
			this.members.push({ workspaceId: workspace.id, userId: user.id, role: 'owner' });
			return this.ok(res, workspace);
		}
		const members = /^\/workspaces\/([^/]+)\/members(?:\/([^/]+))?$/.exec(path);
		if (members?.[1] !== undefined) {
			const workspaceId = members[1];
			if (method === 'GET')
				return this.ok(
					res,
					this.page(
						this.members
							.filter((m) => m.workspaceId === workspaceId)
							.map((m) => ({ user_id: m.userId, role: m.role }))
					)
				);
			if (method === 'POST') {
				this.members.push({ workspaceId, userId: String(body.user_id), role: String(body.role) });
				return this.ok(res, {});
			}
			if (method === 'PATCH' && members[2] !== undefined) {
				const found = this.members.find(
					(m) => m.workspaceId === workspaceId && m.userId === members[2]
				);
				if (found !== undefined) found.role = String(body.role);
				return this.ok(res, {});
			}
		}
		const projects = /^\/workspaces\/([^/]+)\/projects$/.exec(path);
		if (projects?.[1] !== undefined) {
			const workspaceId = projects[1];
			if (method === 'GET')
				return this.ok(
					res,
					this.page([...this.projects.values()].filter((p) => p.workspaceId === workspaceId))
				);
			if (method === 'POST') {
				const project: Project = {
					id: randomUUID(),
					workspaceId,
					key: String(body.key),
					name: String(body.name)
				};
				this.projects.set(project.id, project);
				return this.ok(res, project);
			}
		}
		const participants = /^\/projects\/([^/]+)\/ai-participants$/.exec(path);
		if (participants?.[1] !== undefined && method === 'POST') {
			const key = `${participants[1]}:${String(body.id)}`;
			if (this.participants.has(key)) return this.deny(res, 409, 'ai agent id already exists');
			this.participants.add(key);
			return this.ok(res, {});
		}
		const issuesOf = /^\/projects\/([^/]+)\/issues$/.exec(path);
		if (issuesOf?.[1] !== undefined) {
			const projectId = issuesOf[1];
			if (method === 'GET') {
				const search = url.searchParams.get('search') ?? '';
				return this.ok(
					res,
					this.page(
						[...this.issues.values()]
							.filter((i) => i.projectId === projectId && i.title.includes(search))
							.map((i) => this.issueView(i))
					)
				);
			}
			if (method === 'POST') {
				const issue: Issue = {
					id: randomUUID(),
					projectId,
					title: String(body.title),
					assigneeId: null,
					version: 1
				};
				this.issues.set(issue.id, issue);
				return this.ok(res, this.issueView(issue));
			}
		}
		const issue = /^\/issues\/([^/]+)$/.exec(path);
		if (issue?.[1] !== undefined) {
			const found = this.issues.get(issue[1]);
			if (found === undefined) return this.deny(res, 404, 'not found');
			if (method === 'GET') return this.ok(res, this.issueView(found));
			if (method === 'PUT') return this.updateIssue(res, found, body, user);
		}
		const tasksOf = /^\/projects\/([^/]+)\/ai\/tasks$/.exec(path);
		if (tasksOf?.[1] !== undefined) {
			const projectId = tasksOf[1];
			if (method === 'GET')
				return this.ok(
					res,
					this.page(
						[...this.tasks.values()]
							.filter((t) => t.projectId === projectId)
							.map((t) => ({ id: t.id, status: t.status }))
					)
				);
			if (method === 'POST') return this.createTask(res, projectId, body, user);
		}
		const fail = /^\/ai\/callbacks\/task\/([^/]+)\/fail$/.exec(path);
		if (fail?.[1] !== undefined && method === 'POST') {
			const task = this.tasks.get(fail[1]);
			if (task === undefined) return this.deny(res, 404, 'not found');
			this.setTaskStatus(task, 'failed');
			return this.ok(res, {});
		}
		if (path === '/me/connected-apps' && method === 'GET') {
			return this.ok(res, {
				items: [...this.grants.values()]
					.filter((g) => g.userId === user.id && g.status === 'active')
					.map((g) => this.grantView(g))
			});
		}
		const own = /^\/me\/connected-apps\/([^/]+)$/.exec(path);
		if (own?.[1] !== undefined && method === 'DELETE') {
			const grant = this.grants.get(own[1]);
			if (grant === undefined || grant.userId !== user.id || grant.status !== 'active')
				return this.deny(res, 404, 'not found');
			this.revokeGrant(grant.id, 'user_revoked');
			return this.ok(res, {});
		}
		const wsApps = /^\/workspaces\/([^/]+)\/connected-apps(?:\/([^/]+))?$/.exec(path);
		if (wsApps?.[1] !== undefined) {
			const role = this.roleIn(wsApps[1], user.id);
			if (role !== 'admin' && role !== 'owner')
				return this.deny(res, 403, 'owner or admin required');
			if (method === 'GET' && wsApps[2] === undefined) {
				return this.ok(res, {
					items: [...this.grants.values()]
						.filter((g) => g.workspaceId === wsApps[1] && g.status === 'active')
						.map((g) => this.grantView(g, true))
				});
			}
			if (method === 'DELETE' && wsApps[2] !== undefined) {
				this.revokeGrant(wsApps[2], 'workspace_admin_revoked');
				return this.ok(res, {});
			}
		}
		this.deny(res, 404, 'not found');
	}

	private updateIssue(
		res: ServerResponse,
		issue: Issue,
		body: Record<string, unknown>,
		actor: User
	): void {
		if (typeof body.title === 'string') issue.title = body.title;
		issue.version += 1;
		if (typeof body.assignee_id === 'string') {
			issue.assigneeId = body.assignee_id;
			const project = this.projects.get(issue.projectId);
			if (project !== undefined && body.assignee_id !== actor.id) {
				this.emit('notification.created', project.workspaceId, project.id, body.assignee_id, {
					kind: 'notification',
					notification_id: randomUUID(),
					notification_type: 'assignment',
					related: { issue_id: issue.id, comment_id: null, project_id: project.id },
					is_read: false
				});
			}
		}
		this.ok(res, this.issueView(issue));
	}

	private createTask(
		res: ServerResponse,
		projectId: string,
		body: Record<string, unknown>,
		actor: User
	): void {
		const project = this.projects.get(projectId);
		const role = project === undefined ? undefined : this.roleIn(project.workspaceId, actor.id);
		if (project === undefined || (role !== 'admin' && role !== 'owner'))
			return this.deny(res, 403, 'admin or owner required');
		if (!this.participants.has(`${projectId}:${String(body.ai_participant_id)}`))
			return this.deny(res, 400, 'ai participant is not active in this project');
		const task: Task = {
			id: randomUUID(),
			projectId,
			botId: String(body.ai_participant_id),
			status: 'pending',
			referenceId: typeof body.reference_id === 'string' ? body.reference_id : null
		};
		this.tasks.set(task.id, task);
		this.emit('task.created', project.workspaceId, projectId, null, {
			kind: 'ai_task',
			task_id: task.id,
			task_type: 'issue_assigned',
			status: 'pending',
			attempts: 0,
			max_attempts: 3
		});
		// The worker picks the task up.
		setTimeout(() => this.setTaskStatus(task, 'processing'), 300);
		this.ok(res, { id: task.id, status: 'pending' });
	}

	private setTaskStatus(task: Task, status: string): void {
		const project = this.projects.get(task.projectId);
		if (project === undefined || task.status === status) return;
		const previous = task.status;
		task.status = status;
		this.emit('task.status_changed', project.workspaceId, project.id, null, {
			kind: 'ai_task',
			task_id: task.id,
			status,
			previous_status: previous
		});
	}

	// ---- OAuth: authorization requests, decision, token, revoke ------------------------------------

	private redirectError(
		request: Record<string, string>,
		error: string,
		description?: string
	): string {
		const target = new URL(request.redirect_uri ?? '');
		target.searchParams.set('error', error);
		if (description !== undefined) target.searchParams.set('error_description', description);
		target.searchParams.set('state', request.state ?? '');
		target.searchParams.set('iss', this.ui);
		return target.toString();
	}

	private authorizationRequest(
		req: IncomingMessage,
		res: ServerResponse,
		body: Record<string, unknown>
	): void {
		let stored: AuthRequest | undefined;
		if (typeof body.request_id === 'string') {
			stored = this.requests.get(body.request_id);
			if (stored === undefined)
				return this.ok(res, { kind: 'error_page', error: 'request_expired' });
		} else {
			const params: Record<string, string> = {};
			for (const key of [
				'response_type',
				'client_id',
				'redirect_uri',
				'scope',
				'state',
				'code_challenge',
				'code_challenge_method',
				'device_name'
			]) {
				const value = body[key];
				if (typeof value === 'string') params[key] = value;
			}
			const resources = Array.isArray(body.resource) ? body.resource.map(String) : [];
			if (params.client_id !== 'ramflux' || !this.clientActive)
				return this.ok(res, { kind: 'error_page', error: 'unknown_client' });
			const uri = /^http:\/\/127\.0\.0\.1:(\d+)\/oauth\/sylvode\/callback$/.exec(
				params.redirect_uri ?? ''
			);
			if (uri?.[1] === undefined || Number(uri[1]) < 1024 || Number(uri[1]) > 65535)
				return this.ok(res, { kind: 'error_page', error: 'redirect_uri_mismatch' });
			const bad = (error: string): void =>
				this.ok(res, { kind: 'redirect', redirect_to: this.redirectError(params, error) });
			if (params.response_type !== 'code') return bad('unsupported_response_type');
			if (
				params.code_challenge_method !== 'S256' ||
				!/^[A-Za-z0-9\-._~]{43,128}$/.test(params.code_challenge ?? '')
			)
				return bad('invalid_request');
			if ((params.state ?? '').length < 16) return bad('invalid_request');
			const allowedResources = [`${this.ui}/api`, this.mcpResource];
			if (resources.length === 0 || resources.some((r) => !allowedResources.includes(r)))
				return bad('invalid_target');
			const scopes = (params.scope ?? '').split(' ').filter((s) => s !== '');
			if (scopes.length === 0 || scopes.some((s) => !ALLOWED_SCOPES.has(s)))
				return bad('invalid_scope');
			stored = { id: randomUUID(), params, resources, scopes, consentToken: '', decided: false };
			this.requests.set(stored.id, stored);
		}
		const user = this.sessionUser(req);
		if (user === null)
			return this.ok(res, {
				kind: 'login_required',
				request_id: stored.id,
				client: { client_id: 'ramflux', display_name: 'Ramflux', redirect_host: '127.0.0.1' }
			});
		stored.consentToken = hex(32);
		const eligible = [...this.workspaces.values()].filter(
			(w) => this.roleIn(w.id, user.id) !== undefined
		);
		this.ok(res, {
			kind: 'consent',
			request_id: stored.id,
			consent_token: stored.consentToken,
			client: {
				client_id: 'ramflux',
				display_name: 'Ramflux',
				redirect_host: '127.0.0.1',
				device_name: stored.params.device_name
			},
			account: { user_id: user.id, name: user.name, email: user.email },
			workspaces: eligible.map((w) => ({ id: w.id, name: w.name, eligible: true, reason: null })),
			scopes: stored.scopes.map((name) => ({ name, required: true })),
			resources: ['api', 'mcp']
		});
	}

	private decision(
		req: IncomingMessage,
		res: ServerResponse,
		id: string,
		body: Record<string, unknown>
	): void {
		const request = this.requests.get(id);
		if (request === undefined) return this.deny(res, 404, 'unknown or expired request');
		if (request.decided) return this.deny(res, 409, 'already decided');
		const kind = body.decision;
		if (kind === 'no_account') {
			request.decided = true;
			return this.ok(res, {
				redirect_to: this.redirectError(request.params, 'access_denied', 'account_required')
			});
		}
		const user = this.sessionUser(req);
		if (user === null) return this.deny(res, 401, 'sign in required');
		if (req.headers['x-sylvode-consent'] !== request.consentToken || request.consentToken === '')
			return this.deny(res, 403, 'consent token missing or wrong');
		request.decided = true;
		if (kind === 'deny')
			return this.ok(res, {
				redirect_to: this.redirectError(request.params, 'access_denied', 'consent_denied')
			});
		const workspaceId = String(body.workspace_id);
		if (this.roleIn(workspaceId, user.id) === undefined)
			return this.deny(res, 403, 'workspace not eligible');
		const projectIds = Array.isArray(body.project_ids) ? body.project_ids.map(String) : [];
		const grant: Grant = {
			id: randomUUID(),
			userId: user.id,
			workspaceId,
			projectMode: body.project_mode === 'selected' ? 'selected' : 'all',
			projectIds,
			scopes: Array.isArray(body.scopes) ? body.scopes.map(String) : [],
			status: 'active',
			sv: user.sv,
			deviceName: request.params.device_name ?? ''
		};
		this.grants.set(grant.id, grant);
		const code = `svc_${hex(32)}`;
		this.tokens.set(code, {
			kind: 'code',
			grantId: grant.id,
			revoked: false,
			consumed: false,
			expiresAt: Date.now() + 60_000,
			redirectUri: request.params.redirect_uri,
			challenge: request.params.code_challenge,
			resources: request.resources
		});
		const target = new URL(request.params.redirect_uri ?? '');
		target.searchParams.set('code', code);
		target.searchParams.set('state', request.params.state ?? '');
		target.searchParams.set('iss', this.ui);
		this.ok(res, { redirect_to: target.toString() });
	}

	private oauthError(res: ServerResponse, status: number, error: string): void {
		this.send(
			res,
			status,
			{ error, error_description: error },
			{ 'cache-control': 'no-store', pragma: 'no-cache' }
		);
	}

	private issue(grant: Grant, resource: string, refresh: Tok | undefined): Record<string, unknown> {
		const isApi = resource === `${this.ui}/api`;
		const scopes = grant.scopes.filter((s) => (isApi ? s !== 'offline_access' : MCP_SCOPES.has(s)));
		const access = `sva_${hex(32)}`;
		this.tokens.set(access, {
			kind: 'access',
			grantId: grant.id,
			resource,
			revoked: false,
			consumed: false,
			expiresAt: Date.now() + 600_000
		});
		const body: Record<string, unknown> = {
			access_token: access,
			token_type: 'Bearer',
			expires_in: 600,
			scope: scopes.join(' '),
			grant_id: grant.id,
			workspace_id: grant.workspaceId
		};
		if (refresh !== undefined || grant.scopes.includes('offline_access')) {
			const next = `svr_${hex(32)}`;
			this.tokens.set(next, {
				kind: 'refresh',
				grantId: grant.id,
				revoked: false,
				consumed: false,
				expiresAt: Date.now() + 86_400_000
			});
			body.refresh_token = next;
		}
		return body;
	}

	private token(res: ServerResponse, form: URLSearchParams): void {
		const headers = { 'cache-control': 'no-store', pragma: 'no-cache' };
		const grantType = form.get('grant_type');
		if (form.get('client_id') !== 'ramflux') return this.oauthError(res, 401, 'invalid_client');
		if (grantType === 'authorization_code') {
			const code = this.tokens.get(form.get('code') ?? '');
			if (code === undefined || code.kind !== 'code' || code.expiresAt < Date.now())
				return this.oauthError(res, 400, 'invalid_grant');
			if (code.consumed) {
				this.revokeGrant(code.grantId, 'code_replay');
				return this.oauthError(res, 400, 'invalid_grant');
			}
			const verifier = form.get('code_verifier') ?? '';
			if (form.get('redirect_uri') !== code.redirectUri || sha(verifier) !== code.challenge)
				return this.oauthError(res, 400, 'invalid_grant');
			const resource = form.get('resource') ?? '';
			if (!(code.resources ?? []).includes(resource))
				return this.oauthError(res, 400, 'invalid_target');
			const grant = this.grants.get(code.grantId);
			if (grant === undefined || grant.status !== 'active')
				return this.oauthError(res, 400, 'invalid_grant');
			code.consumed = true;
			return this.send(res, 200, this.issue(grant, resource, undefined), headers);
		}
		if (grantType === 'refresh_token') {
			const refresh = this.tokens.get(form.get('refresh_token') ?? '');
			if (refresh === undefined || refresh.kind !== 'refresh')
				return this.oauthError(res, 400, 'invalid_grant');
			const grant = this.grants.get(refresh.grantId);
			if (refresh.consumed && this.opts.fault !== 'refresh-reuse') {
				this.revokeGrant(refresh.grantId, 'refresh_reuse');
				return this.oauthError(res, 400, 'invalid_grant');
			}
			if (refresh.revoked || grant === undefined || grant.status !== 'active')
				return this.oauthError(res, 400, 'invalid_grant');
			const resource = form.get('resource') ?? '';
			if (resource !== `${this.ui}/api` && resource !== this.mcpResource)
				return this.oauthError(res, 400, 'invalid_target');
			refresh.consumed = true;
			return this.send(res, 200, this.issue(grant, resource, refresh), headers);
		}
		this.oauthError(res, 400, 'unsupported_grant_type');
	}

	private revoke(res: ServerResponse, form: URLSearchParams): void {
		const token = this.tokens.get(form.get('token') ?? '');
		if (token !== undefined) {
			if (token.kind === 'refresh') this.revokeGrant(token.grantId, 'client_revoked');
			else token.revoked = true;
		}
		res.writeHead(200, { 'cache-control': 'no-store' });
		res.end();
	}

	// ---- the delegated API ---------------------------------------------------------------------------

	private validate(
		token: string,
		resource: string
	): { grant: Grant; tok: Tok } | 'invalid' | 'audience' {
		const tok = this.tokens.get(token);
		if (tok === undefined || tok.kind !== 'access' || tok.revoked || tok.expiresAt < Date.now())
			return 'invalid';
		const grant = this.grants.get(tok.grantId);
		const user = grant === undefined ? undefined : this.users.get(grant.userId);
		if (
			grant === undefined ||
			grant.status !== 'active' ||
			user === undefined ||
			!user.active ||
			user.sv !== grant.sv ||
			!this.clientActive
		)
			return 'invalid';
		if (tok.resource !== resource) return 'audience';
		return { grant, tok };
	}

	private delegated(
		req: IncomingMessage,
		res: ServerResponse,
		url: URL,
		method: string,
		path: string,
		body: Record<string, unknown>,
		token: string
	): void {
		void body;
		const checked = this.validate(token, `${this.ui}/api`);
		if (checked === 'invalid')
			return this.bearerFail(
				res,
				401,
				'invalid_token',
				'the access token is missing, invalid or expired'
			);
		if (checked === 'audience')
			return this.bearerFail(res, 401, 'invalid_token', 'token audience does not match');
		const { grant, tok } = checked;
		const has = (scope: string): boolean =>
			grant.scopes.includes(scope) ||
			(scope === 'work_items:read' && grant.scopes.includes('work_items:write'));
		if (path === '/oauth/connection' && method === 'GET') {
			const projects = grant.projectIds
				.map((id) => this.projects.get(id))
				.filter((p): p is Project => p !== undefined);
			const user = this.users.get(grant.userId);
			return this.ok(res, {
				grant_id: grant.id,
				client_id: 'ramflux',
				workspace: { id: grant.workspaceId, name: this.workspaces.get(grant.workspaceId)?.name },
				account: { user_id: grant.userId, name: user?.name, email: user?.email },
				scopes: grant.scopes,
				project_mode: grant.projectMode,
				projects: projects.map((p) => ({ id: p.id, key: p.key, name: p.name }))
			});
		}
		if (path === '/oauth/events/stream' && method === 'GET')
			return this.openStream(req, res, url, grant, token, tok);
		const issue = /^\/issues\/([^/]+)$/.exec(path);
		if (issue?.[1] !== undefined) {
			const need = method === 'PUT' ? 'work_items:write' : 'work_items:read';
			if (!has(need))
				return this.bearerFail(
					res,
					403,
					'insufficient_scope',
					'the connection does not include the required scope',
					need === 'work_items:write' ? 'work_items:read work_items:write' : need
				);
			const found = this.issues.get(issue[1]);
			const reachable =
				found !== undefined &&
				(grant.projectMode === 'all' || grant.projectIds.includes(found.projectId));
			if (!reachable || found === undefined) {
				return this.send(res, 403, {
					code: 403,
					message: 'the requested object is not available to this connection',
					error_code: 'forbidden',
					error: 'forbidden'
				});
			}
			return this.ok(res, this.issueView(found));
		}
		this.bearerFail(
			res,
			403,
			'insufficient_scope',
			'route not available to connected applications'
		);
	}

	private openStream(
		req: IncomingMessage,
		res: ServerResponse,
		url: URL,
		grant: Grant,
		token: string,
		tok: Tok
	): void {
		void token;
		const typesParam = url.searchParams.get('types');
		const types = typesParam === null ? null : new Set(typesParam.split(','));
		res.writeHead(200, {
			'content-type': 'text/event-stream; charset=utf-8',
			'cache-control': 'no-store',
			'x-accel-buffering': 'no'
		});
		const stream: Stream = {
			res,
			grantId: grant.id,
			tokenId: token,
			types,
			timer: setInterval(() => res.write(': keep-alive\n\n'), 4000)
		};
		this.streams.add(stream);
		res.on('close', () => {
			clearInterval(stream.timer);
			this.streams.delete(stream);
		});
		const last = req.headers['last-event-id'];
		const resumeFrom =
			typeof last === 'string' ? Number(/^cur-(\d+)$/.exec(last)?.[1] ?? Number.NaN) : Number.NaN;
		const startAfter = Number.isNaN(resumeFrom) ? this.seq : resumeFrom;
		this.sendControl(stream, {
			type: 'hello',
			grant_id: grant.id,
			workspace_id: grant.workspaceId,
			cursor: this.cursor(this.seq),
			server_time: new Date().toISOString()
		});
		for (const event of this.feed) if (event.seq > startAfter) this.deliver(stream, event);
		// The stream never outlives its token.
		setTimeout(
			() => {
				if (this.streams.has(stream)) {
					this.sendControl(stream, { type: 'reauth', reason: 'token_expired' });
					this.closeStream(stream);
				}
			},
			Math.max(1000, tok.expiresAt - Date.now())
		);
	}

	// ---- the MCP resource -------------------------------------------------------------------------------

	private async handleMcp(req: IncomingMessage, res: ServerResponse): Promise<void> {
		const url = new URL(req.url ?? '/', this.mcp);
		if (req.method === 'GET' && url.pathname === '/.well-known/oauth-protected-resource') {
			return this.send(res, 200, { resource: this.mcpResource, authorization_servers: [this.ui] });
		}
		if (req.method === 'GET' && url.pathname === '/health') {
			res.writeHead(200);
			return void res.end('ok');
		}
		if (req.method !== 'POST' || url.pathname !== '/mcp/rpc') {
			res.writeHead(404);
			return void res.end();
		}
		const raw = await this.body(req);
		const header = req.headers.authorization;
		const token = header?.startsWith('Bearer ') ? header.slice(7) : '';
		const challenge = `Bearer realm="sylvode", error="invalid_token", resource_metadata="${this.mcp}/.well-known/oauth-protected-resource"`;
		const checked = token.startsWith('sva_') ? this.validate(token, this.mcpResource) : 'invalid';
		if (checked === 'invalid' || checked === 'audience') {
			return this.send(res, 401, { error: 'invalid_token' }, { 'www-authenticate': challenge });
		}
		const { grant } = checked;
		const rpc = JSON.parse(raw) as {
			id?: unknown;
			method?: string;
			params?: { name?: string; arguments?: Record<string, unknown> };
		};
		const reply = (result: unknown): void =>
			this.send(res, 200, { jsonrpc: '2.0', id: rpc.id, result });
		if (rpc.method === 'tools/list')
			return reply({
				tools: grant.scopes.includes('projects:read') ? READ_TOOLS.map((name) => ({ name })) : []
			});
		if (rpc.method === 'tools/call') {
			const name = rpc.params?.name ?? '';
			if (!READ_TOOLS.includes(name)) {
				return this.send(res, 403, {
					jsonrpc: '2.0',
					id: rpc.id,
					error: { code: -32002, message: 'tool not available to connected applications' }
				});
			}
			const found = this.issues.get(String(rpc.params?.arguments?.work_item_id ?? ''));
			if (
				found === undefined ||
				!(grant.projectMode === 'all' || grant.projectIds.includes(found.projectId))
			) {
				return reply({
					content: [
						{
							type: 'text',
							text: 'Error: the requested object is not available to this connection'
						}
					],
					isError: true
				});
			}
			return reply({
				content: [{ type: 'text', text: JSON.stringify(this.issueView(found), null, 2) }]
			});
		}
		this.send(res, 200, {
			jsonrpc: '2.0',
			id: rpc.id,
			error: { code: -32601, message: 'method not found' }
		});
	}
}
