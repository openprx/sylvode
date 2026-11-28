/** Synthetic transport only: no real credentials, cookies, user data or held live scenarios. */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { apiClient } from '../src/lib/api/client';
function deferred<T>() {
	let resolve!: (value: T) => void;
	return { promise: new Promise<T>((done) => (resolve = done)), resolve };
}
const envelope = (code: number, data: unknown = null) =>
	new Response(JSON.stringify({ code, message: 'synthetic fixture', data }), {
		headers: { 'Content-Type': 'application/json' }
	});
test('delayed old write 401 cannot refresh, retry, clear or replace a newer account session', async () => {
	const original = globalThis.fetch,
		pending = deferred<Response>();
	let writes = 0,
		refreshes = 0;
	apiClient.setToken('synthetic-access-a');
	apiClient.setRefreshToken('synthetic-refresh-a');
	globalThis.fetch = async (input) => {
		if (String(input).endsWith('/api/v1/auth/refresh')) {
			refreshes++;
			return envelope(0, {
				tokens: {
					access_token: 'synthetic-access-b-refreshed',
					refresh_token: 'synthetic-refresh-b-refreshed'
				}
			});
		}
		writes++;
		return writes === 1 ? pending.promise : envelope(0, { accepted: true });
	};
	try {
		const request = apiClient.patch('/synthetic-old-write', { value: 'old draft' });
		apiClient.setToken('synthetic-access-b');
		apiClient.setRefreshToken('synthetic-refresh-b');
		pending.resolve(envelope(401));
		const result = await request;
		assert.equal(result.code, 499);
		assert.equal(refreshes, 0);
		assert.equal(writes, 1);
		assert.equal(apiClient.getToken(), 'synthetic-access-b');
		assert.equal(apiClient.getRefreshToken(), 'synthetic-refresh-b');
	} finally {
		globalThis.fetch = original;
		apiClient.clearAuth();
	}
});

const jwt = (sub: string, n = 1) =>
	`synthetic.${btoa(JSON.stringify({ sub, n })).replace(/=/g, '')}.signature`;
async function until(check: () => boolean) {
	for (let i = 0; i < 30 && !check(); i++) await new Promise((resolve) => setTimeout(resolve, 0));
	assert(check());
}
test('same-account concurrent 401s share one refresh and preserve the owning epoch', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch,
		refresh = deferred<Response>();
	let refreshes = 0;
	const counts = new Map<string, number>();
	client.installAuthSession(jwt('actor-a'), 'synthetic-refresh-a', 'actor-a');
	const epoch = client.getAuthSessionEpoch();
	globalThis.fetch = async (input) => {
		const path = String(input);
		if (path.endsWith('/auth/refresh')) {
			refreshes++;
			return refresh.promise;
		}
		const count = (counts.get(path) ?? 0) + 1;
		counts.set(path, count);
		return envelope(count === 1 ? 401 : 0, { accepted: true });
	};
	try {
		const first = client.patch('/synthetic-one', {}),
			second = client.patch('/synthetic-two', {});
		await until(() => refreshes === 1);
		refresh.resolve(
			envelope(0, {
				user: { id: 'actor-a' },
				tokens: { access_token: jwt('actor-a', 2), refresh_token: 'synthetic-refresh-a2' }
			})
		);
		assert.equal((await first).code, 0);
		assert.equal((await second).code, 0);
		assert.equal(refreshes, 1);
		assert.equal(client.getAuthSessionEpoch(), epoch);
		assert.equal(client.getToken(), jwt('actor-a', 2));
		assert.equal(counts.get('/synthetic-one'), 2);
		assert.equal(counts.get('/synthetic-two'), 2);
	} finally {
		globalThis.fetch = original;
		client.dispose();
	}
});
test('a late old refresh cannot resurrect logout or clear a newer single-flight refresh', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch,
		a = deferred<Response>(),
		b = deferred<Response>();
	let refreshes = 0;
	let oldSignal: AbortSignal | undefined;
	globalThis.fetch = async (_input, options) => {
		refreshes++;
		if (refreshes === 1) {
			oldSignal = options?.signal as AbortSignal;
			return a.promise;
		}
		return b.promise;
	};
	try {
		client.installAuthSession(jwt('actor-a'), 'synthetic-a', 'actor-a');
		const old = client.ensureFreshAccessToken();
		await until(() => refreshes === 1);
		client.clearAuth();
		client.installAuthSession(jwt('actor-b'), 'synthetic-b', 'actor-b');
		const current = client.ensureFreshAccessToken();
		await until(() => refreshes === 2);
		assert(oldSignal?.aborted);
		a.resolve(
			envelope(0, {
				user: { id: 'actor-a' },
				tokens: { access_token: jwt('actor-a', 2), refresh_token: 'synthetic-a2' }
			})
		);
		assert.equal(await old, false);
		const joined = client.ensureFreshAccessToken();
		assert.equal(refreshes, 2);
		b.resolve(
			envelope(0, {
				user: { id: 'actor-b' },
				tokens: { access_token: jwt('actor-b', 2), refresh_token: 'synthetic-b2' }
			})
		);
		assert(await current);
		assert(await joined);
		assert.equal(client.getToken(), jwt('actor-b', 2));
		client.clearAuth();
		assert.equal(client.getToken(), null);
	} finally {
		globalThis.fetch = original;
		client.dispose();
	}
});
test('an old refresh that finishes after logout alone cannot reinstall credentials', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch,
		late = deferred<Response>();
	globalThis.fetch = async () => late.promise;
	try {
		client.installAuthSession(jwt('actor-a'), 'synthetic-a', 'actor-a');
		const request = client.ensureFreshAccessToken();
		client.clearAuth();
		late.resolve(
			envelope(0, {
				user: { id: 'actor-a' },
				tokens: { access_token: jwt('actor-a', 2), refresh_token: 'synthetic-a2' }
			})
		);
		assert.equal(await request, false);
		assert.equal(client.getToken(), null);
		assert.equal(client.getRefreshToken(), null);
	} finally {
		globalThis.fetch = original;
		client.dispose();
	}
});
test('actual login/logout adapter provenance and delayed session verification do not mutate a newer user', async () => {
	const { authApi } = await import('../src/lib/api/auth');
	const { authStore, verifySession } = await import('../src/lib/stores/auth');
	const { get } = await import('svelte/store');
	const original = globalThis.fetch,
		late = deferred<Response>();
	const user = (id: string) => ({
		id,
		email: 'synthetic@example.invalid',
		name: id,
		created_at: '2026-01-01'
	});
	globalThis.fetch = async () => late.promise;
	try {
		apiClient.installAuthSession(jwt('actor-a'), 'synthetic-a', 'actor-a');
		authStore.setUser(user('actor-a'));
		const logout = authApi.logout();
		apiClient.installAuthSession(jwt('actor-b'), 'synthetic-b', 'actor-b');
		authStore.setUser(user('actor-b'));
		late.resolve(envelope(0));
		const result = await logout;
		assert.equal(result.code, 499);
		assert.equal(apiClient.isAuthResultCurrent(result), false);
		if (apiClient.isAuthResultCurrent(result)) await authStore.logout();
		assert.equal(get(authStore).user?.id, 'actor-b');
		assert.equal(apiClient.getToken(), jwt('actor-b'));
		const answer =
			deferred<import('../src/lib/api/client').ApiResult<{ user: ReturnType<typeof user> }>>();
		const check = verifySession(() => answer.promise);
		apiClient.installAuthSession(jwt('actor-c'), 'synthetic-c', 'actor-c');
		authStore.setUser(user('actor-c'));
		answer.resolve({ code: 401, message: 'synthetic old check', data: null });
		assert.equal(await check, 'superseded');
		assert.equal(get(authStore).user?.id, 'actor-c');
		assert.equal(apiClient.getToken(), jwt('actor-c'));
	} finally {
		globalThis.fetch = original;
		authStore.endSession();
	}
});
test('competing login intents install only the newest session and tag the actual UI continuation', async () => {
	const { authApi } = await import('../src/lib/api/auth');
	const original = globalThis.fetch,
		a = deferred<Response>(),
		b = deferred<Response>();
	let calls = 0;
	globalThis.fetch = async () => (++calls === 1 ? a.promise : b.promise);
	const data = (id: string) => ({
		user: { id, email: 'synthetic@example.invalid', name: id, created_at: '2026-01-01' },
		tokens: {
			access_token: jwt(id),
			refresh_token: `synthetic-${id}`,
			token_type: 'Bearer',
			access_expires_in: 60,
			refresh_expires_in: 600
		}
	});
	try {
		const first = authApi.login({ email: 'a@example.invalid', password: 'synthetic' }),
			second = authApi.login({ email: 'b@example.invalid', password: 'synthetic' });
		b.resolve(envelope(0, data('actor-b')));
		const accepted = await second;
		a.resolve(envelope(0, data('actor-a')));
		const stale = await first;
		assert.equal(accepted.code, 0);
		assert.equal(stale.code, 499);
		assert(apiClient.isAuthResultCurrent(accepted));
		assert.equal(apiClient.isAuthResultCurrent(stale), false);
		assert.equal(apiClient.getToken(), jwt('actor-b'));
		assert.equal(calls, 2);
	} finally {
		globalThis.fetch = original;
		apiClient.clearAuth();
	}
});
test('real storage event wiring invalidates changed/unknown principals but preserves same-actor refresh epoch', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const { createAuthStore } = await import('../src/lib/stores/auth');
	const { get } = await import('svelte/store');
	const oldWindow = Object.getOwnPropertyDescriptor(globalThis, 'window'),
		oldStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage'),
		original = globalThis.fetch;
	const data = new Map<string, string>();
	const storage = {
		getItem: (key: string) => data.get(key) ?? null,
		setItem: (key: string, value: string) => data.set(key, value),
		removeItem: (key: string) => data.delete(key)
	};
	const target = Object.assign(new EventTarget(), {
		location: { href: '', pathname: '/synthetic' }
	});
	Object.defineProperty(globalThis, 'window', { configurable: true, value: target });
	Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage });
	const profile = (id: string) =>
		JSON.stringify({ id, email: 'synthetic@example.invalid', name: id, created_at: '2026-01-01' });
	data.set('auth_token', jwt('actor-a'));
	data.set('refresh_token', 'synthetic-a');
	data.set('auth_user', profile('actor-a'));
	const client = new ApiClient(''),
		store = createAuthStore(client),
		pending = deferred<Response>();
	let requests = 0;
	globalThis.fetch = async () => {
		requests++;
		return pending.promise;
	};
	const fire = (key: string) => target.dispatchEvent(Object.assign(new Event('storage'), { key }));
	try {
		const before = client.getAuthSessionEpoch(),
			old = client.patch('/synthetic-old-write', {});
		data.set('auth_token', jwt('actor-b'));
		data.set('refresh_token', 'synthetic-b');
		data.set('auth_user', profile('actor-b'));
		fire('auth_user');
		assert(client.getAuthSessionEpoch() > before);
		assert.equal(get(store).user?.id, 'actor-b');
		pending.resolve(envelope(401));
		assert.equal((await old).code, 499);
		assert.equal(requests, 1);
		const same = client.getAuthSessionEpoch();
		data.set('auth_token', jwt('actor-b', 2));
		data.set('refresh_token', 'synthetic-b2');
		fire('auth_token');
		assert.equal(client.getAuthSessionEpoch(), same);
		assert.equal(get(store).user?.id, 'actor-b');
		data.set('auth_token', 'unidentifiable-synthetic-token');
		fire('auth_token');
		assert(client.getAuthSessionEpoch() > same);
		assert.equal(get(store).user, null);
		assert.equal(data.get('auth_token'), 'unidentifiable-synthetic-token');
	} finally {
		store.dispose();
		client.dispose();
		globalThis.fetch = original;
		if (oldWindow) Object.defineProperty(globalThis, 'window', oldWindow);
		else Reflect.deleteProperty(globalThis, 'window');
		if (oldStorage) Object.defineProperty(globalThis, 'localStorage', oldStorage);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
});
test('late binary response is discarded after actor changes without refresh or new-account clearing', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch,
		pending = deferred<Response>();
	let calls = 0;
	globalThis.fetch = async () => {
		calls++;
		return pending.promise;
	};
	try {
		client.installAuthSession(jwt('actor-a'), 'synthetic-a', 'actor-a');
		const old = client.getBinary('/synthetic-bytes');
		client.installAuthSession(jwt('actor-b'), 'synthetic-b', 'actor-b');
		pending.resolve(envelope(401));
		assert.equal((await old).code, 499);
		assert.equal(calls, 1);
		assert.equal(client.getToken(), jwt('actor-b'));
	} finally {
		globalThis.fetch = original;
		client.dispose();
	}
});

test('late XHR upload callbacks cannot refresh or report progress into a new actor session', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		descriptor = Object.getOwnPropertyDescriptor(globalThis, 'XMLHttpRequest'),
		original = globalThis.fetch;
	let refreshes = 0,
		progress = 0;
	class SyntheticXHR {
		static current: SyntheticXHR;
		withCredentials = false;
		responseText = '';
		upload: {
			onprogress:
				| ((value: { loaded: number; total: number; lengthComputable: boolean }) => void)
				| null;
		} = { onprogress: null };
		onload: (() => void) | null = null;
		onerror: (() => void) | null = null;
		onabort: (() => void) | null = null;
		open() {
			SyntheticXHR.current = this;
		}
		setRequestHeader() {}
		send() {}
		abort() {
			this.onabort?.();
		}
	}
	Object.defineProperty(globalThis, 'XMLHttpRequest', { configurable: true, value: SyntheticXHR });
	globalThis.fetch = async () => {
		refreshes++;
		return envelope(0);
	};
	try {
		client.installAuthSession(jwt('actor-a'), 'synthetic-a', 'actor-a');
		const body = new FormData();
		body.set('file', new File(['synthetic'], 'fixture.txt'));
		const request = client.postFormDataWithProgress(
			'/synthetic-upload',
			body,
			{},
			undefined,
			() => progress++
		);
		const xhr = SyntheticXHR.current;
		client.installAuthSession(jwt('actor-b'), 'synthetic-b', 'actor-b');
		xhr.upload.onprogress?.({ loaded: 5, total: 5, lengthComputable: true });
		xhr.responseText = JSON.stringify({ code: 401, message: 'synthetic', data: null });
		xhr.onload?.();
		assert.equal((await request).code, 499);
		assert.equal(refreshes, 0);
		assert.equal(progress, 0);
		assert.equal(client.getToken(), jwt('actor-b'));
	} finally {
		globalThis.fetch = original;
		client.dispose();
		if (descriptor) Object.defineProperty(globalThis, 'XMLHttpRequest', descriptor);
		else Reflect.deleteProperty(globalThis, 'XMLHttpRequest');
	}
});
test('a session check cut short by the page unloading does not end the stored session', async () => {
	const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
	const target = new EventTarget();
	const storageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	const entries = new Map<string, string>();
	Object.defineProperty(globalThis, 'window', { value: target, configurable: true });
	Object.defineProperty(globalThis, 'localStorage', {
		value: {
			getItem: (key: string) => entries.get(key) ?? null,
			setItem: (key: string, value: string) => void entries.set(key, value),
			removeItem: (key: string) => void entries.delete(key)
		},
		configurable: true
	});
	try {
		// A second instance of the module, loaded while `window` exists, registers the listeners.
		const fresh = '../src/lib/stores/auth?unload-fixture';
		const { verifySession } = (await import(
			/* @vite-ignore */ fresh
		)) as typeof import('../src/lib/stores/auth');
		const failed = { code: 500, message: 'synthetic aborted check', data: null };
		apiClient.installAuthSession(jwt('actor-u'), 'synthetic-u', 'actor-u');
		target.dispatchEvent(new Event('pagehide'));
		assert.equal(await verifySession(async () => failed), 'superseded');
		assert.equal(apiClient.getToken(), jwt('actor-u'));
		target.dispatchEvent(new Event('pageshow'));
		assert.equal(await verifySession(async () => failed), 'ended');
		assert.equal(apiClient.getToken(), null);
	} finally {
		if (windowDescriptor) Object.defineProperty(globalThis, 'window', windowDescriptor);
		else Reflect.deleteProperty(globalThis, 'window');
		if (storageDescriptor) Object.defineProperty(globalThis, 'localStorage', storageDescriptor);
		else Reflect.deleteProperty(globalThis, 'localStorage');
		apiClient.clearAuth();
	}
});

const tooMany = (retryAfter: string) =>
	new Response(JSON.stringify({ code: 429, message: 'too many requests; wait', data: null }), {
		status: 429,
		headers: { 'Content-Type': 'application/json', 'Retry-After': retryAfter }
	});
test('a 429 from refresh is temporary: it waits as asked, retries and keeps the session', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch;
	let refreshes = 0,
		writes = 0;
	client.installAuthSession(jwt('actor-a'), 'synthetic-refresh-a', 'actor-a');
	globalThis.fetch = async (input) => {
		if (String(input).endsWith('/auth/refresh')) {
			refreshes++;
			return refreshes === 1
				? tooMany('1')
				: envelope(0, {
						tokens: { access_token: jwt('actor-a', 2), refresh_token: 'synthetic-refresh-a2' }
					});
		}
		writes++;
		return envelope(writes === 1 ? 401 : 0, { accepted: true });
	};
	try {
		const result = await client.patch('/synthetic-write', {});
		assert.equal(result.code, 0);
		assert.equal(refreshes, 2);
		assert.equal(client.getRefreshToken(), 'synthetic-refresh-a2');
	} finally {
		globalThis.fetch = original;
		client.clearAuth();
	}
});
test('a refresh that stays 429 does not sign the person out', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch;
	let refreshes = 0;
	client.installAuthSession(jwt('actor-a'), 'synthetic-refresh-a', 'actor-a');
	globalThis.fetch = async (input) => {
		if (String(input).endsWith('/auth/refresh')) {
			refreshes++;
			return tooMany('1');
		}
		return envelope(401);
	};
	try {
		const result = await client.patch('/synthetic-write', {});
		assert.equal(result.code, 401);
		assert.equal(refreshes, 3, 'one try and two retries');
		assert.equal(client.getRefreshToken(), 'synthetic-refresh-a', 'the session is kept');
		assert.notEqual(client.getToken(), null);
	} finally {
		globalThis.fetch = original;
		client.clearAuth();
	}
});
test('a refused refresh (401) still ends the session', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch;
	client.installAuthSession(jwt('actor-a'), 'synthetic-refresh-a', 'actor-a');
	globalThis.fetch = async () => envelope(401);
	try {
		await client.patch('/synthetic-write', {});
		assert.equal(client.getToken(), null);
		assert.equal(client.getRefreshToken(), null);
	} finally {
		globalThis.fetch = original;
		client.clearAuth();
	}
});
test('a 429 answer carries a translatable message and the wait', async () => {
	const { ApiClient } = await import('../src/lib/api/client');
	const client = new ApiClient(''),
		original = globalThis.fetch;
	globalThis.fetch = async () => tooMany('42');
	try {
		const result = await client.post('/api/v1/auth/login', {});
		assert.equal(result.code, 429);
		assert.equal(result.messageKey, 'transport.rateLimited');
		assert.deepEqual(result.messageValues, { seconds: 42 });
		assert.equal(result.message, '');
	} finally {
		globalThis.fetch = original;
	}
});
