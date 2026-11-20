// The first-party Sylvode API as a signed-in person (JWT), used for setup, for the actions the
// scenarios attribute to user B or to the instance administrator, and for state checks.

import type { Account, Config } from '../config.ts';
import { call, type HttpResult } from './http.ts';
import { redactor } from './redact.ts';
import type { Recorder } from './record.ts';
import { asArray, asRecord, check, dig, str } from './util.ts';

export function envelopeData(res: HttpResult, label: string): unknown {
	check(res.status === 200, `${label}: HTTP ${res.status}`);
	const body = asRecord(res.json);
	check(body !== null, `${label}: the body is not a JSON object`);
	check(
		body.code === 0,
		`${label}: envelope code ${JSON.stringify(body.code)} (${str(body.message) ?? 'no message'})`
	);
	return body.data;
}

/** The `items` of a paged envelope, or a bare array. */
export function itemsOf(data: unknown): unknown[] {
	return asArray(data) ?? asArray(asRecord(data)?.items) ?? [];
}

export class FirstParty {
	readonly role: string;
	readonly account: Account;
	private readonly cfg: Config;
	private jwt: string | undefined;
	private id: string | undefined;

	constructor(cfg: Config, role: string, account: Account) {
		this.cfg = cfg;
		this.role = role;
		this.account = account;
		redactor.register(account.password, `password:${role}`);
	}

	invalidate(): void {
		this.jwt = undefined;
	}

	/** The person's user id; signs in when needed. */
	async userId(rec: Recorder): Promise<string> {
		await this.login(rec);
		check(this.id !== undefined, `${this.role}: sign-in returned no user id`);
		return this.id;
	}

	async login(rec: Recorder): Promise<string> {
		if (this.jwt !== undefined) return this.jwt;
		const res = await this.loginWith(rec, this.account.password);
		const data = envelopeData(res, `${this.role} sign-in`);
		const token = str(dig(data, 'tokens', 'access_token'));
		const id = str(dig(data, 'user', 'id'));
		check(
			token !== undefined && id !== undefined,
			`${this.role} sign-in: no access token or user id in the answer`
		);
		redactor.register(token, 'jwt');
		this.jwt = token;
		this.id = id;
		return token;
	}

	/** Signs in with an explicit password (used to prove the old one stopped working). */
	loginWith(rec: Recorder, password: string): Promise<HttpResult> {
		return call({
			rec,
			label: `${this.role} sign-in`,
			method: 'POST',
			url: `${this.cfg.apiOrigin}/api/v1/auth/login`,
			json: { email: this.account.email, password }
		});
	}

	async request(
		rec: Recorder,
		label: string,
		method: string,
		path: string,
		body?: unknown
	): Promise<HttpResult> {
		const attempt = async (): Promise<HttpResult> =>
			call({
				rec,
				label,
				method,
				url: `${this.cfg.apiOrigin}${path}`,
				bearer: await this.login(rec),
				json: body
			});
		const first = await attempt();
		// The API answers a refused session as HTTP 200 with code 401 in the envelope, or as 401.
		if (first.status !== 401 && asRecord(first.json)?.code !== 401) return first;
		// The session may have been invalidated on purpose (password change, account disabled): once.
		this.invalidate();
		return attempt();
	}

	async data(
		rec: Recorder,
		label: string,
		method: string,
		path: string,
		body?: unknown
	): Promise<unknown> {
		return envelopeData(await this.request(rec, label, method, path, body), label);
	}
}
