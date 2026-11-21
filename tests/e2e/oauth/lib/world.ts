// The shared state of a run: who the people are, where the workspace and projects are, the
// discovery result, and the helpers every case builds on (connecting a grant through the real
// consent page, acting as user B, opening streams). One World per Playwright worker.

import type { Browser } from '@playwright/test';
import { type Config } from '../config.ts';
import { CALLBACK_PATH, CallbackListener, parseCallback } from './callback.ts';
import { driveConsent, type ConsentChoice, type ConsentView } from './consent.ts';
import { call, type HttpResult } from './http.ts';
import { codeChallengeS256, newState, newVerifier } from './pkce.ts';
import { redactor } from './redact.ts';
import { Recorder } from './record.ts';
import { EventStream, type OpenResult } from './stream.ts';
import { envelopeData, FirstParty, itemsOf } from './sv.ts';
import {
	buildAuthorizeUrl,
	Connection,
	discover,
	exchangeCode,
	type Discovery,
	type OAuthEnv,
	type TokenResponse,
	REFERENCE_SCOPE
} from './oauth-client.ts';
import { asRecord, check, errorMessage, RollingLimiter, str } from './util.ts';

export interface Ids {
	workspaceId: string;
	p1Id: string;
	p2Id: string;
	botId: string;
	userAId: string;
	userBId: string;
	adminId: string;
}

/** What one case holds while it runs: its recorder, its browser contexts and streams. */
export class CaseCtx {
	readonly id: string;
	readonly rec = new Recorder();
	readonly world: World;
	readonly browser: Browser;
	private readonly cleanups: Array<{ name: string; run: () => Promise<void> }> = [];
	private readonly streams: EventStream[] = [];
	/** Names of cleanups that failed; a case whose restore failed is a failed case. */
	readonly cleanupFailures: string[] = [];

	constructor(id: string, world: World, browser: Browser) {
		this.id = id;
		this.world = world;
		this.browser = browser;
	}

	onCleanup(name: string, run: () => Promise<void>): void {
		this.cleanups.push({ name, run });
	}

	timing(name: string, ms: number): void {
		this.rec.timings[name] = Math.round(ms);
	}

	detail(name: string, value: unknown): void {
		this.rec.details[name] = value;
	}

	/** Opens a stream and keeps it for cleanup and for the failure transcript. */
	async openStream(
		conn: Connection,
		label: string,
		opts: { token?: string; lastEventId?: string; types?: string } = {}
	): Promise<OpenResult> {
		await this.world.streamLimiter(conn.grantId).wait();
		const token = opts.token ?? (await conn.access(this.rec, 'api'));
		const result = await EventStream.open({
			rec: this.rec,
			label,
			url: this.world.streamUrl(),
			token,
			lastEventId: opts.lastEventId,
			types: opts.types
		});
		if (result.stream !== null) this.streams.push(result.stream);
		return result;
	}

	/** Opens a stream that must open, and waits for its hello. */
	async openStreamOrFail(
		conn: Connection,
		label: string,
		opts: { token?: string; lastEventId?: string; types?: string } = {}
	): Promise<EventStream> {
		const result = await this.openStream(conn, label, opts);
		check(result.stream !== null, `${label}: the stream did not open (HTTP ${result.status})`);
		const hello = await result.stream.waitForControl('hello', 10_000);
		check(hello !== null, `${label}: no hello control message within 10 s`);
		return result.stream;
	}

	/** Runs the cleanups (last registered first) and closes the streams; never throws. */
	async finish(): Promise<void> {
		for (const stream of this.streams) {
			this.rec.note(`stream "${stream.label}" transcript:`);
			for (const line of stream.transcript()) this.rec.note(`  ${line}`);
			await stream.close().catch(() => undefined);
		}
		for (const cleanup of this.cleanups.reverse()) {
			try {
				await cleanup.run();
			} catch (error) {
				this.rec.note(`cleanup "${cleanup.name}" failed: ${redactor.text(errorMessage(error))}`);
				this.cleanupFailures.push(cleanup.name);
			}
		}
	}
}

export interface ConnectOptions {
	scope?: string;
	projects: 'all' | { names: string[] };
	deviceName?: string;
}

export interface Connected {
	conn: Connection;
	grantId: string;
	workspaceId: string;
	/** Kept in memory for the code replay case. */
	authorization: { code: string; redirectUri: string; verifier: string };
	/** The two token responses, for the cases that assert on their fields. */
	mcpTokens: TokenResponse;
	apiTokens: TokenResponse;
}

export class World {
	readonly cfg: Config;
	readonly admin: FirstParty;
	readonly userA: FirstParty;
	readonly userB: FirstParty;
	readonly worldRec = new Recorder();
	ids!: Ids;
	disc!: Discovery;
	private discoveryError: string | undefined;
	private shared: Connection | undefined;
	private readonly flowLimiter: RollingLimiter;
	private readonly streamLimiters = new Map<string, RollingLimiter>();

	private constructor(cfg: Config) {
		this.cfg = cfg;
		this.flowLimiter = new RollingLimiter(cfg.flowsPerMinute, 60_000);
		this.admin = new FirstParty(cfg, 'admin', cfg.admin);
		this.userA = new FirstParty(cfg, 'userA', cfg.userA);
		this.userB = new FirstParty(cfg, 'userB', cfg.userB);
	}

	static async create(cfg: Config): Promise<World> {
		const world = new World(cfg);
		const rec = world.worldRec;
		try {
			world.disc = await discover(cfg, rec);
		} catch (error) {
			world.discoveryError = redactor.text(errorMessage(error));
		}
		world.ids = await world.resolveIds(rec);
		await world.ensureClientActive(rec);
		await world.revokeAllOfA(rec);
		return world;
	}

	env(): OAuthEnv {
		check(this.discoveryError === undefined, `discovery failed: ${this.discoveryError ?? ''}`);
		return { cfg: this.cfg, disc: this.disc };
	}

	streamLimiter(grantId: string): RollingLimiter {
		let limiter = this.streamLimiters.get(grantId);
		if (limiter === undefined) {
			limiter = new RollingLimiter(8, 60_000);
			this.streamLimiters.set(grantId, limiter);
		}
		return limiter;
	}

	delegatedUrl(path: string): string {
		return `${this.env().disc.apiResource}/v1${path}`;
	}

	streamUrl(): string {
		return this.delegatedUrl('/oauth/events/stream');
	}

	// ---- ids ----------------------------------------------------------------------------------

	private async resolveIds(rec: Recorder): Promise<Ids> {
		const cfg = this.cfg;
		const workspaces = itemsOf(
			await this.admin.data(rec, 'list workspaces', 'GET', '/api/v1/workspaces')
		);
		const workspace = workspaces
			.map(asRecord)
			.find((w) =>
				cfg.pinned.workspaceId !== undefined
					? str(w?.id) === cfg.pinned.workspaceId
					: str(w?.name) === cfg.workspaceName
			);
		const workspaceId = str(workspace?.id);
		check(
			workspaceId !== undefined && str(workspace?.name)?.startsWith('qa-p4-') === true,
			`workspace "${cfg.workspaceName}" not found for the administrator (run the setup first)`
		);

		const projects = itemsOf(
			await this.admin.data(
				rec,
				'list projects',
				'GET',
				`/api/v1/workspaces/${workspaceId}/projects?per_page=100`
			)
		).map(asRecord);
		const projectId = (key: string, pinned: string | undefined): string => {
			const found = projects.find((p) =>
				pinned !== undefined ? str(p?.id) === pinned : str(p?.key) === key
			);
			const id = str(found?.id);
			check(
				id !== undefined,
				`project with key ${key} not found in the workspace (run the setup first)`
			);
			return id;
		};
		const p1Id = projectId(cfg.p1Key, cfg.pinned.p1Id);
		const p2Id = projectId(cfg.p2Key, cfg.pinned.p2Id);

		const members = itemsOf(
			await this.admin.data(
				rec,
				'list members',
				'GET',
				`/api/v1/workspaces/${workspaceId}/members?per_page=100`
			)
		).map(asRecord);
		const userAId = await this.userA.userId(rec);
		const userBId = await this.userB.userId(rec);
		const adminId = await this.admin.userId(rec);
		const roleOf = (id: string): string | undefined =>
			str(members.find((m) => str(m?.user_id) === id)?.role);
		check(
			roleOf(userAId) !== undefined,
			'user A is not a member of the workspace (run the setup first)'
		);
		const roleB = roleOf(userBId);
		check(
			roleB === 'admin' || roleB === 'owner',
			`user B must be an admin of the workspace (is: ${roleB ?? 'not a member'}); run the setup first`
		);

		let botId = cfg.pinned.botId;
		if (botId === undefined) {
			const found = itemsOf(
				await this.admin.data(
					rec,
					'find the bot',
					'GET',
					`/api/v1/admin/users?search=${encodeURIComponent(cfg.botEmail)}&limit=20`
				)
			).map(asRecord);
			botId = str(
				found.find((u) => str(u?.email)?.toLowerCase() === cfg.botEmail.toLowerCase())?.id
			);
		}
		check(
			botId !== undefined && roleOf(botId) !== undefined,
			'the bot user is missing or not a member of the workspace (run the setup first)'
		);
		return { workspaceId, p1Id, p2Id, botId, userAId, userBId, adminId };
	}

	// ---- state that must be restored ------------------------------------------------------------

	async ensureClientActive(rec: Recorder): Promise<void> {
		const res = await this.admin.request(
			rec,
			'enable the client',
			'PATCH',
			`/api/v1/admin/oauth-clients/${this.cfg.clientId}`,
			{ status: 'active' }
		);
		check(
			res.status === 200 && asRecord(res.json)?.code === 0,
			`could not make sure the client is active (HTTP ${res.status})`
		);
	}

	async revokeAllOfA(rec: Recorder): Promise<void> {
		const list = itemsOf(
			await this.userA.data(
				rec,
				'list own connected applications',
				'GET',
				'/api/v1/me/connected-apps'
			)
		);
		for (const item of list) {
			const id = str(asRecord(item)?.grant_id);
			if (id !== undefined)
				await this.userA.request(
					rec,
					'disconnect a leftover connection',
					'DELETE',
					`/api/v1/me/connected-apps/${id}`
				);
		}
		this.shared = undefined;
	}

	/** Makes sure user A can sign in with the configured password and is active. */
	async restoreUserA(rec: Recorder): Promise<void> {
		const id = this.ids.userAId;
		const enabled = await this.admin.request(
			rec,
			'enable user A',
			'PATCH',
			`/api/v1/admin/users/${id}/status`,
			{ is_active: true }
		);
		check(
			enabled.status === 200 && asRecord(enabled.json)?.code === 0,
			`could not re-enable user A (HTTP ${enabled.status})`
		);
		const attempt = await this.userA.loginWith(rec, this.cfg.userA.password);
		if (attempt.status !== 200 || asRecord(attempt.json)?.code !== 0) {
			const reset = await this.admin.request(
				rec,
				'reset the password of user A',
				'PUT',
				`/api/v1/admin/users/${id}/password`,
				{ new_password: this.cfg.userA.password }
			);
			check(
				reset.status === 200 && asRecord(reset.json)?.code === 0,
				`could not restore the password of user A (HTTP ${reset.status})`
			);
		}
		this.userA.invalidate();
	}

	async teardown(): Promise<void> {
		const rec = new Recorder();
		const problems: string[] = [];
		for (const [name, step] of [
			['restore user A', () => this.restoreUserA(rec)],
			['enable the client', () => this.ensureClientActive(rec)],
			['disconnect leftover connections', () => this.revokeAllOfA(rec)]
		] as const) {
			try {
				await step();
			} catch (error) {
				problems.push(`${name}: ${redactor.text(errorMessage(error))}`);
			}
		}
		if (problems.length > 0) throw new Error(`teardown problems: ${problems.join('; ')}`);
	}

	// ---- acting as B and the administrator ------------------------------------------------------

	async createItem(rec: Recorder, projectId: string, title: string): Promise<string> {
		const data = await this.userB.data(
			rec,
			'B creates a work item',
			'POST',
			`/api/v1/projects/${projectId}/issues`,
			{ title }
		);
		const id = str(asRecord(data)?.id);
		check(id !== undefined, 'creating a work item returned no id');
		return id;
	}

	/** B assigns an item to A; returns the monotonic time at which the response had arrived. */
	async assignToA(rec: Recorder, itemId: string): Promise<number> {
		const res = await this.userB.request(
			rec,
			'B assigns the item to A',
			'PUT',
			`/api/v1/issues/${itemId}`,
			{ assignee_id: this.ids.userAId }
		);
		envelopeData(res, 'B assigns the item to A');
		return res.doneAt;
	}

	async getIssue(rec: Recorder, itemId: string): Promise<Record<string, unknown>> {
		const data = asRecord(
			await this.userB.data(rec, 'B reads a work item', 'GET', `/api/v1/issues/${itemId}`)
		);
		check(data !== null, 'the work item answer is not an object');
		return data;
	}

	/** The seed work item of a project (created by the setup); one is created when it is missing. */
	async seedItem(rec: Recorder, which: 'p1' | 'p2'): Promise<{ id: string; title: string }> {
		const projectId = which === 'p1' ? this.ids.p1Id : this.ids.p2Id;
		const listed = itemsOf(
			await this.userB.data(
				rec,
				'B lists work items',
				'GET',
				`/api/v1/projects/${projectId}/issues?per_page=50&search=qa-p4-seed`
			)
		).map(asRecord);
		const found = listed.find((item) => str(item?.title)?.startsWith('qa-p4-seed') === true);
		const foundId = str(found?.id);
		if (foundId !== undefined) return { id: foundId, title: str(found?.title) ?? '' };
		const title = `qa-p4-seed-${which}`;
		return { id: await this.createItem(rec, projectId, title), title };
	}

	// ---- connecting a grant ---------------------------------------------------------------------

	/**
	 * Starts an authorization as the client does and takes the person through the consent page in
	 * a fresh browser context. Returns once the decision was made; the redirect is then waited for
	 * on `listener`.
	 */
	async authorize(
		c: CaseCtx,
		choice: ConsentChoice,
		extra: { scope?: string; redirectHost?: string; deviceName?: string; drive?: boolean } = {}
	): Promise<{
		listener: CallbackListener;
		state: string;
		verifier: string;
		redirectUri: string;
		view: ConsentView;
		errorText: string;
	}> {
		const env = this.env();
		await this.flowLimiter.wait();
		const listener = await CallbackListener.start();
		c.onCleanup('callback listener', () => listener.close());
		const verifier = newVerifier();
		redactor.register(verifier, 'verifier');
		const state = newState();
		const redirectUri = `http://${extra.redirectHost ?? '127.0.0.1'}:${listener.port}${CALLBACK_PATH}`;
		const url = buildAuthorizeUrl(env, {
			redirectUri,
			scope: extra.scope ?? REFERENCE_SCOPE,
			state,
			challenge: codeChallengeS256(verifier),
			resources: [env.disc.apiResource, env.disc.mcpResource],
			deviceName: extra.deviceName ?? 'qa-p4 runner'
		});
		const context = await c.browser.newContext({
			locale: 'en-US',
			viewport: { width: 1280, height: 900 }
		});
		c.onCleanup('browser context', () => context.close());
		const page = await context.newPage();
		if (extra.drive === false) {
			// A request the server answers with a redirect at once (no page to drive).
			await page.goto(url, { waitUntil: 'load' }).catch(() => undefined);
			return { listener, state, verifier, redirectUri, view: 'other', errorText: '' };
		}
		const driven = await driveConsent(page, url, choice);
		return {
			listener,
			state,
			verifier,
			redirectUri,
			view: driven.view,
			errorText: driven.errorText
		};
	}

	/** A full connection as the client does it: browser consent, redirect check, both exchanges. */
	async connect(c: CaseCtx, options: ConnectOptions): Promise<Connected> {
		const env = this.env();
		const started = await this.authorize(
			c,
			{
				mode: 'allow',
				login: this.cfg.userA,
				workspaceId: this.ids.workspaceId,
				projects: options.projects
			},
			{ scope: options.scope, deviceName: options.deviceName }
		);
		let target: string;
		try {
			target = await started.listener.waitForCallback(30_000);
		} catch (error) {
			c.rec.note(
				`consent page after the decision: ${started.errorText === '' ? '(no error text)' : started.errorText}`
			);
			throw error;
		}
		const verdict = parseCallback(target, {
			path: CALLBACK_PATH,
			iss: this.cfg.uiOrigin,
			state: started.state
		});
		check(
			verdict.ok,
			`the redirect was not a clean grant: ${JSON.stringify(redactor.json(verdict))}`
		);
		redactor.register(verdict.code, 'code');
		const { verifier, redirectUri } = started;

		const exchanged = await exchangeCode(env, c.rec, {
			code: verdict.code,
			redirectUri,
			verifier,
			resource: env.disc.mcpResource,
			label: 'token: code for the MCP resource'
		});
		check(
			exchanged.accessToken !== undefined &&
				exchanged.grantId !== undefined &&
				exchanged.workspaceId !== undefined,
			`the code exchange failed (HTTP ${exchanged.res.status}, ${exchanged.error ?? 'no error code'})`
		);
		check(
			exchanged.refreshToken !== undefined,
			'the code exchange returned no refresh token although offline_access was granted'
		);
		const conn = new Connection(env, {
			grantId: exchanged.grantId,
			workspaceId: exchanged.workspaceId,
			refreshToken: exchanged.refreshToken
		});
		conn.hold('mcp', exchanged.accessToken, exchanged.expiresIn ?? 600);
		const forApi = await conn.rotate(c.rec, 'api', 'token: refresh for the API resource');
		check(
			forApi.accessToken !== undefined,
			`the refresh for the API resource failed (HTTP ${forApi.res.status}, ${forApi.error ?? 'no error code'})`
		);
		return {
			conn,
			grantId: exchanged.grantId,
			workspaceId: exchanged.workspaceId,
			authorization: { code: verdict.code, redirectUri, verifier },
			mcpTokens: exchanged,
			apiTokens: forApi
		};
	}

	/** The grant most cases share: reference scopes, projects {P1}. Replaced when it died. */
	async sharedConnection(c: CaseCtx): Promise<Connection> {
		if (this.shared !== undefined) {
			try {
				const token = await this.shared.access(c.rec, 'api');
				const res = await call({
					rec: c.rec,
					label: 'probe the shared connection',
					method: 'GET',
					url: this.delegatedUrl('/oauth/connection'),
					bearer: token
				});
				if (res.status === 200) return this.shared;
			} catch {
				// fall through to a fresh grant
			}
			this.shared = undefined;
		}
		const connected = await this.connect(c, { projects: { names: [this.cfg.p1Name] } });
		this.shared = connected.conn;
		return connected.conn;
	}

	forgetShared(): void {
		this.shared = undefined;
	}

	/** Lets the connection a case just made serve as the shared one. */
	adoptShared(conn: Connection): void {
		this.shared = conn;
	}
}
