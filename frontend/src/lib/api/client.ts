import { loginUrlForLocation } from '#lib/oauth/return-to.js';
import { transportFailure, type TransportKey } from './transport';

export type { TransportKey } from './transport';
export { transportFailure } from './transport';

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '';

export interface ApiResult<T> {
	code: number;
	/** Prose the server sent; '' for a failure the client itself produced (see `messageKey`). */
	message: string;
	/** Locale key (`transport.*`) of a failure the client itself produced. Modules never format
	 * text, so they run without an initialised locale; `describeResult` resolves the key. */
	messageKey?: string;
	messageValues?: Record<string, string | number>;
	data: T | null;
	/** The stable, machine-readable business code for a typed rejection
	 * (`apps/api/src/error.rs::ApiErrorKind::stable_code`; `contracts/error-mapping-v1.md`'s
	 * "稳定错误的五层映射"). Present only on responses the API produced through `ApiError::Typed`;
	 * absent on success and on the legacy string-typed errors. Callers MUST branch on this and
	 * never on `message` -- `message` is localized prose and is explicitly not a discriminator
	 * ("禁止用英文 message 分支"). */
	error_code?: string;
	/** The typed rejection's structured `details` (e.g. `server_draining`'s required
	 * `{reason,retry_after_ms}`, `limit_exceeded`'s `{limit_kind,limit,observed}`). Carried
	 * verbatim: dropping it here is what makes a required discriminator unreadable to every UI
	 * surface downstream. */
	details?: unknown;
}

/** Upload progress for a request body (`XMLHttpRequest.upload.onprogress`). `total` is `null`
 * when the browser cannot compute it. */
export interface UploadProgress {
	loaded: number;
	total: number | null;
}

export interface PaginatedData<T> {
	items: T[];
	total: number;
	page: number;
	per_page: number;
	total_pages: number;
}

function normalizeEnvelope<T>(parsed: Partial<ApiResult<T>>): ApiResult<T> {
	return {
		code: typeof parsed.code === 'number' ? parsed.code : 500,
		message: typeof parsed.message === 'string' ? parsed.message : '',
		...(typeof parsed.message === 'string' ? {} : { messageKey: 'transport.invalidResponse' }),
		data: (parsed.data as T | null) ?? null,
		...(typeof parsed.error_code === 'string' ? { error_code: parsed.error_code } : {}),
		...(parsed.details === undefined ? {} : { details: parsed.details })
	};
}

/** Longest wait the client honours for one `Retry-After`, in seconds. */
const MAX_RETRY_AFTER_SECS = 15;
/** How many times a refresh that was answered 429 is tried again. */
const REFRESH_RETRIES = 2;

/** The seconds a 429 asks to wait (`Retry-After`), at least 1; 5 when the header is missing. */
export function retryAfterSecs(res: Pick<Response, 'headers'>): number {
	const seconds = Number.parseInt(res.headers.get('Retry-After') ?? '', 10);
	return Number.isFinite(seconds) && seconds > 0 ? seconds : 5;
}

/** A 429 as a result the UI can word in the user's language, with the wait it asks for. */
function rateLimited<T>(res: Pick<Response, 'headers'>): ApiResult<T> {
	const seconds = retryAfterSecs(res);
	return {
		...transportFailure<T>(429, 'transport.rateLimited', {
			error_code: 'rate_limited',
			details: { retry_after_secs: seconds }
		}),
		messageValues: { seconds }
	};
}

/** What a token refresh came to: a new token, a refusal (sign in again), or a temporary failure
 * that leaves the session as it is. */
export type RefreshOutcome = 'refreshed' | 'denied' | 'unavailable';

/** Local cancellation only: an earlier legitimate server mutation may already have applied. */
export function authSessionChanged<T>(): ApiResult<T> {
	return transportFailure<T>(499, 'transport.sessionChanged', {
		error_code: 'request_aborted',
		details: { reason: 'auth_session_changed', outcome: 'unknown' }
	});
}

/** Used only to isolate client ownership; the server still verifies every token and permission. */
export function authPrincipal(token: string | null): string | null {
	if (!token) return null;
	try {
		const part = token.split('.')[1];
		if (!part) return null;
		const value = JSON.parse(atob(part.replace(/-/g, '+').replace(/_/g, '/')));
		return typeof value.sub === 'string' && value.sub.length ? value.sub : null;
	} catch {
		return null;
	}
}
export interface ExternalAuthSnapshot {
	principal: string | null;
	profile: unknown;
	consistent: boolean;
}

export class ApiClient {
	private baseUrl: string;
	private token: string | null = null;
	private refreshTokenValue: string | null = null;
	private refreshInFlight: { epoch: number; promise: Promise<RefreshOutcome> } | null = null;
	private authEpoch = 0;
	private epochAbort = new AbortController();
	private principal: string | null = null;
	private results = new WeakMap<object, number>();
	private external = new Set<(value: ExternalAuthSnapshot) => void>();
	private readonly storageChanged = (event: StorageEvent) => {
		if (event.key === null || ['auth_token', 'refresh_token', 'auth_user'].includes(event.key))
			this.syncStoredAuth();
	};

	constructor(baseUrl: string = API_BASE_URL) {
		this.baseUrl = baseUrl;
		// Follow-up: stop persisting access/refresh tokens in localStorage. The API already
		// sets HttpOnly cookies, but the documented dev server talks to the API on another
		// origin (`VITE_API_BASE_URL`), and credentialed cross-origin cookies need an explicit
		// CORS allow-list plus a non-HttpOnly CSRF cookie checked on cookie-authenticated
		// mutations. Until that lands, Bearer tokens remain in localStorage and any XSS on
		// this origin can read them. SameSite=Lax cookies are still set alongside.
		if (typeof window !== 'undefined') {
			this.token = localStorage.getItem('auth_token');
			this.refreshTokenValue = localStorage.getItem('refresh_token');
			this.principal = authPrincipal(this.token);
			window.addEventListener('storage', this.storageChanged);
		}
	}

	setToken(token: string | null) {
		this.beginAuthSession();
		this.principal = authPrincipal(token);
		this.writeToken(token);
	}
	private writeToken(token: string | null): void {
		this.token = token;
		if (typeof window !== 'undefined') {
			if (token) {
				localStorage.setItem('auth_token', token);
			} else {
				localStorage.removeItem('auth_token');
			}
		}
	}

	clearToken() {
		this.setToken(null);
	}

	setRefreshToken(token: string | null) {
		this.beginAuthSession();
		this.writeRefreshToken(token);
	}
	private writeRefreshToken(token: string | null): void {
		this.refreshTokenValue = token;
		if (typeof window !== 'undefined') {
			if (token) {
				localStorage.setItem('refresh_token', token);
			} else {
				localStorage.removeItem('refresh_token');
			}
		}
	}

	getRefreshToken(): string | null {
		return this.refreshTokenValue;
	}

	clearAuth() {
		this.beginAuthSession();
		this.principal = null;
		this.writeToken(null);
		this.writeRefreshToken(null);
	}
	beginAuthSession(): number {
		this.authEpoch++;
		this.epochAbort.abort();
		this.epochAbort = new AbortController();
		return this.authEpoch;
	}
	authSessionSignal(epoch = this.authEpoch): AbortSignal {
		return this.isAuthSessionEpoch(epoch) ? this.epochAbort.signal : AbortSignal.abort();
	}

	getAuthSessionEpoch(): number {
		return this.authEpoch;
	}
	isAuthSessionEpoch(epoch: number): boolean {
		return epoch === this.authEpoch;
	}
	installAuthSession(access: string, refresh: string | null, principal?: string): void {
		this.beginAuthSession();
		this.principal = principal ?? authPrincipal(access);
		this.writeToken(access);
		this.writeRefreshToken(refresh);
	}
	bindAuthActor(principal: string | null): void {
		if (this.principal !== principal) {
			this.beginAuthSession();
			this.principal = principal;
		}
	}
	getAuthPrincipal(): string | null {
		return this.principal;
	}
	bindAuthResult<T>(result: ApiResult<T>, epoch = this.authEpoch): ApiResult<T> {
		this.results.set(result, epoch);
		return result;
	}
	isAuthResultCurrent(result: object, fallbackEpoch?: number): boolean {
		return (this.results.get(result) ?? fallbackEpoch) === this.authEpoch;
	}
	onExternalAuth(listener: (value: ExternalAuthSnapshot) => void): () => void {
		this.external.add(listener);
		return () => this.external.delete(listener);
	}
	private syncStoredAuth(): void {
		const access = localStorage.getItem('auth_token'),
			refresh = localStorage.getItem('refresh_token');
		let profile: unknown = null;
		try {
			profile = JSON.parse(localStorage.getItem('auth_user') ?? 'null');
		} catch {
			/* Unknown identity aborts old work. */
		}
		const principal = authPrincipal(access),
			profileId =
				profile && typeof profile === 'object' && 'id' in profile
					? (profile as { id: unknown }).id
					: null;
		const consistent = !!principal && principal === profileId;
		if (!consistent || principal !== this.principal) this.beginAuthSession();
		this.principal = principal;
		this.token = access;
		this.refreshTokenValue = refresh;
		for (const listener of this.external) listener({ principal, profile, consistent });
	}
	dispose(): void {
		this.epochAbort.abort();
		this.external.clear();
		if (typeof window !== 'undefined') window.removeEventListener('storage', this.storageChanged);
	}

	private requestSignal(signal?: AbortSignal): AbortSignal {
		return signal ? AbortSignal.any([this.epochAbort.signal, signal]) : this.epochAbort.signal;
	}

	private redirectToLogin() {
		if (typeof window !== 'undefined') {
			// An expired session on the consent page comes back to the stored request after sign-in.
			window.location.href = loginUrlForLocation(window.location.pathname, window.location.search);
		}
	}

	private async performTokenRefresh(epoch: number): Promise<RefreshOutcome> {
		if (!this.isAuthSessionEpoch(epoch)) return 'denied';
		const signal = this.epochAbort.signal;
		const url = `${this.baseUrl}/api/v1/auth/refresh`;

		for (let attempt = 0; ; attempt++) {
			const refreshToken = this.getRefreshToken();
			const body = refreshToken ? JSON.stringify({ refresh_token: refreshToken }) : undefined;
			const headers = new Headers();
			headers.set('Content-Type', 'application/json');
			try {
				const res = await fetch(url, {
					method: 'POST',
					headers,
					body,
					signal,
					credentials: 'include'
				});
				// Too many requests is temporary: keep the session, wait as asked (bounded) and try
				// again; when it persists the session stays and the caller's own request fails.
				if (res.status === 429) {
					if (attempt >= REFRESH_RETRIES) return 'unavailable';
					const waitMs = Math.min(retryAfterSecs(res), MAX_RETRY_AFTER_SECS) * 1000;
					await new Promise<void>((done) => setTimeout(done, waitMs));
					if (!this.isAuthSessionEpoch(epoch) || signal.aborted) return 'denied';
					continue;
				}
				if (res.status >= 500) return 'unavailable';
				const parsed = (await res.json()) as Partial<
					ApiResult<{
						tokens?: { access_token?: string; refresh_token?: string };
						user?: { id?: string };
					}>
				>;

				if (
					this.isAuthSessionEpoch(epoch) &&
					!signal.aborted &&
					parsed.code === 0 &&
					parsed.data?.tokens?.access_token &&
					typeof parsed.data.tokens.access_token === 'string'
				) {
					const refreshedPrincipal =
						parsed.data.user?.id ?? authPrincipal(parsed.data.tokens.access_token);
					if (this.principal && refreshedPrincipal && this.principal !== refreshedPrincipal)
						return 'denied';
					this.principal = this.principal ?? refreshedPrincipal ?? null;
					this.writeToken(parsed.data.tokens.access_token);
					if (typeof parsed.data.tokens.refresh_token === 'string') {
						this.writeRefreshToken(parsed.data.tokens.refresh_token);
					}
					return 'refreshed';
				}
				// The server's own 429 or 5xx in the envelope is as temporary as the HTTP status.
				if (parsed.code === 429 || (typeof parsed.code === 'number' && parsed.code >= 500))
					return 'unavailable';
			} catch {
				return 'denied';
			}
			return 'denied';
		}
	}

	private async refreshAccessTokenOnce(epoch = this.authEpoch): Promise<RefreshOutcome> {
		if (!this.isAuthSessionEpoch(epoch)) return 'denied';
		if (this.refreshInFlight?.epoch === epoch) return this.refreshInFlight.promise;
		const flight: { epoch: number; promise: Promise<RefreshOutcome> } = {
			epoch,
			promise: Promise.resolve('denied')
		};
		flight.promise = this.performTokenRefresh(epoch).finally(() => {
			if (this.refreshInFlight === flight) this.refreshInFlight = null;
		});
		this.refreshInFlight = flight;
		return flight.promise;
	}

	getToken(): string | null {
		return this.token;
	}

	/**
	 * Public single-flight access-token refresh (`contracts/ui-surface-v1.md` "连接、refresh 与
	 * 恢复状态机" step 1/3: "`ApiClient` 暴露 public `ensureFreshAccessToken(): Promise<boolean>`,
	 * 内部继续复用源码已有 single-flight `refreshInFlight`"). Callers outside this module -- e.g.
	 * `ObjectSession` retrying a WebSocket collab ticket exactly once after a 401 -- use this
	 * instead of duplicating refresh logic or racing `request()`'s own internal retry, since both
	 * paths share the same `refreshInFlight` promise and never issue two concurrent refreshes.
	 * Resolves `true` only when the refresh actually produced a new access token.
	 */
	async ensureFreshAccessToken(epoch = this.authEpoch): Promise<boolean> {
		return (await this.refreshAccessToken(epoch)) === 'refreshed';
	}

	/**
	 * Like `ensureFreshAccessToken`, but says why it failed: `denied` means sign in again,
	 * `unavailable` means the refresh was answered 429 or 5xx and the session is intact, so the
	 * caller should try again later instead of treating the person as signed out.
	 */
	async refreshAccessToken(epoch = this.authEpoch): Promise<RefreshOutcome> {
		return this.refreshAccessTokenOnce(epoch);
	}

	async request<T>(
		method: string,
		endpoint: string,
		body?: unknown,
		retryAfterRefresh: boolean = true,
		additionalHeaders: Record<string, string> = {},
		signal?: AbortSignal
	): Promise<ApiResult<T>> {
		const epoch = this.authEpoch;
		const requestSignal = this.requestSignal(signal);
		const url = `${this.baseUrl}${endpoint}`;
		const headers = new Headers();
		const isFormData = typeof FormData !== 'undefined' && body instanceof FormData;
		const isBlob = typeof Blob !== 'undefined' && body instanceof Blob;
		if (!isFormData) headers.set('Content-Type', isBlob ? body.type : 'application/json');
		if (this.token) {
			headers.set('Authorization', `Bearer ${this.token}`);
		}
		for (const [name, value] of Object.entries(additionalHeaders)) headers.set(name, value);

		try {
			const res = await fetch(url, {
				method,
				headers,
				body: isFormData || isBlob ? body : body ? JSON.stringify(body) : undefined,
				signal: requestSignal,
				credentials: 'include'
			});

			if (!this.isAuthSessionEpoch(epoch))
				return this.bindAuthResult(authSessionChanged<T>(), epoch);
			const parsed = (await res.json()) as Partial<ApiResult<T>>;
			const result = res.status === 429 ? rateLimited<T>(res) : normalizeEnvelope<T>(parsed);
			if (!this.isAuthSessionEpoch(epoch))
				return this.bindAuthResult(authSessionChanged<T>(), epoch);

			if (
				result.code === 401 &&
				!['/api/v1/auth/login', '/api/v1/auth/logout'].includes(endpoint)
			) {
				const isRefreshEndpoint = endpoint === '/api/v1/auth/refresh';
				if (!isRefreshEndpoint && retryAfterRefresh) {
					const refreshed = await this.refreshAccessTokenOnce(epoch);
					if (!this.isAuthSessionEpoch(epoch))
						return this.bindAuthResult(authSessionChanged<T>(), epoch);
					if (signal?.aborted)
						return transportFailure<T>(499, 'transport.requestAborted', {
							error_code: 'request_aborted',
							details: { reason: 'request_cancelled', outcome: 'unknown' }
						});
					if (refreshed === 'refreshed') {
						return this.request<T>(method, endpoint, body, false, additionalHeaders, signal);
					}
					// Only a refused refresh ends the session; a temporary failure keeps it.
					if (refreshed === 'unavailable') return this.bindAuthResult(result);
				}
				if (!this.isAuthSessionEpoch(epoch))
					return this.bindAuthResult(authSessionChanged<T>(), epoch);
				this.clearAuth();
				this.redirectToLogin();
			}

			return this.bindAuthResult(result);
		} catch {
			if (!this.isAuthSessionEpoch(epoch))
				return this.bindAuthResult(authSessionChanged<T>(), epoch);
			return transportFailure<T>(500, 'transport.networkError');
		}
	}

	get<T>(endpoint: string): Promise<ApiResult<T>> {
		return this.request<T>('GET', endpoint);
	}

	post<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('POST', endpoint, data);
	}

	postFormData<T>(
		endpoint: string,
		data: FormData,
		headers: Record<string, string> = {},
		signal?: AbortSignal
	): Promise<ApiResult<T>> {
		return this.request<T>('POST', endpoint, data, true, headers, signal);
	}

	/**
	 * `postFormData` with upload progress. `fetch` cannot report request-body progress, so when
	 * the runtime has `XMLHttpRequest` and the caller asked for progress this goes over XHR
	 * (same base URL, bearer token, cookies, extra headers, abort signal, envelope normalisation
	 * and single 401 refresh-and-retry as `request`). Runtimes without `XMLHttpRequest` (Bun unit
	 * tests, SSR) fall back to `request`, which reports no progress.
	 */
	postFormDataWithProgress<T>(
		endpoint: string,
		data: FormData,
		headers: Record<string, string> = {},
		signal?: AbortSignal,
		onProgress?: (progress: UploadProgress) => void
	): Promise<ApiResult<T>> {
		if (!onProgress || typeof XMLHttpRequest === 'undefined') {
			return this.request<T>('POST', endpoint, data, true, headers, signal);
		}
		return this.xhrFormData<T>(endpoint, data, headers, signal, onProgress, true);
	}

	private async xhrFormData<T>(
		endpoint: string,
		data: FormData,
		headers: Record<string, string>,
		signal: AbortSignal | undefined,
		onProgress: (progress: UploadProgress) => void,
		retryAfterRefresh: boolean
	): Promise<ApiResult<T>> {
		const epoch = this.authEpoch;
		const requestSignal = this.requestSignal(signal);
		const result = await new Promise<ApiResult<T>>((resolve) => {
			const failed = (key: TransportKey): ApiResult<T> => transportFailure<T>(500, key);
			if (requestSignal.aborted) {
				resolve(failed('transport.requestAborted'));
				return;
			}
			const xhr = new XMLHttpRequest();
			xhr.open('POST', `${this.baseUrl}${endpoint}`);
			xhr.withCredentials = true;
			if (this.token) xhr.setRequestHeader('Authorization', `Bearer ${this.token}`);
			for (const [name, value] of Object.entries(headers)) xhr.setRequestHeader(name, value);
			xhr.upload.onprogress = (event) => {
				if (this.isAuthSessionEpoch(epoch))
					onProgress({ loaded: event.loaded, total: event.lengthComputable ? event.total : null });
			};
			const onAbort = () => xhr.abort();
			requestSignal.addEventListener('abort', onAbort, { once: true });
			const settle = (value: ApiResult<T>) => {
				requestSignal.removeEventListener('abort', onAbort);
				resolve(value);
			};
			xhr.onload = () => {
				try {
					settle(normalizeEnvelope<T>(JSON.parse(xhr.responseText) as Partial<ApiResult<T>>));
				} catch {
					settle(failed('transport.invalidResponse'));
				}
			};
			xhr.onerror = () => settle(failed('transport.networkError'));
			xhr.onabort = () => settle(failed('transport.requestAborted'));
			xhr.send(data);
		});
		if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged<T>();
		if (result.code === 401 && retryAfterRefresh) {
			const refreshed = await this.refreshAccessTokenOnce(epoch);
			if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged<T>();
			if (refreshed === 'refreshed') {
				return this.xhrFormData<T>(endpoint, data, headers, signal, onProgress, false);
			}
			if (refreshed === 'unavailable') return result;
			this.clearAuth();
			this.redirectToLogin();
		} else if (result.code === 401) {
			this.clearAuth();
			this.redirectToLogin();
		}
		return result;
	}

	/**
	 * GET a binary body with the same auth as `request` (bearer token + cookies, one 401
	 * refresh-and-retry). A JSON response is a business rejection and comes back as its normalised
	 * envelope with `data: null`; anything else is returned as a `Blob` plus the response headers.
	 */
	getBinary(
		endpoint: string,
		retryAfterRefresh = true,
		signal?: AbortSignal
	): Promise<ApiResult<{ blob: Blob; headers: Headers }>> {
		return this.requestBinary(endpoint, 'GET', undefined, retryAfterRefresh, signal);
	}

	postBinary(endpoint: string, input: unknown, signal?: AbortSignal) {
		return this.requestBinary(endpoint, 'POST', JSON.stringify(input), true, signal);
	}

	private async requestBinary(
		endpoint: string,
		method: 'GET' | 'POST',
		body: string | undefined,
		retryAfterRefresh: boolean,
		signal?: AbortSignal
	): Promise<ApiResult<{ blob: Blob; headers: Headers }>> {
		const epoch = this.authEpoch;
		const requestSignal = this.requestSignal(signal);
		const headers = new Headers();
		if (this.token) headers.set('Authorization', `Bearer ${this.token}`);
		if (body !== undefined) headers.set('Content-Type', 'application/json');
		try {
			const res = await fetch(`${this.baseUrl}${endpoint}`, {
				method,
				body,
				signal: requestSignal,
				headers,
				credentials: 'include'
			});
			if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged();
			const contentType = res.headers.get('Content-Type') ?? '';
			// JSON is also a legitimate export format. Only the declared Flow artifact contract
			// distinguishes those raw bytes from the platform's HTTP-200 error envelopes.
			const attachment = /^attachment(?:;|$)/i.test(res.headers.get('Content-Disposition') ?? '');
			const declaredFlowArtifact =
				/^\/api\/v[12]\/flow\/exports\/[^/?#]+\/artifact$/.test(endpoint) &&
				attachment &&
				/^[a-f0-9]{64}$/i.test(res.headers.get('x-flow-package-sha256') ?? '');
			// The project export answers the file itself; its failures are envelopes without a disposition.
			const declaredProjectExport =
				/^\/api\/v1\/projects\/[^/?#]+\/export(?:\?|$)/.test(endpoint) && attachment;
			if (
				contentType.startsWith('application/json') &&
				!declaredFlowArtifact &&
				!declaredProjectExport
			) {
				const result = normalizeEnvelope<{ blob: Blob; headers: Headers }>(
					(await res.json()) as Partial<ApiResult<{ blob: Blob; headers: Headers }>>
				);
				if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged();
				if (result.code === 401) {
					const refreshed = retryAfterRefresh ? await this.refreshAccessTokenOnce(epoch) : 'denied';
					if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged();
					if (refreshed === 'refreshed') {
						return this.requestBinary(endpoint, method, body, false, signal);
					}
					if (refreshed !== 'unavailable') {
						this.clearAuth();
						this.redirectToLogin();
					}
				}
				return { ...result, data: null };
			}
			if (!res.ok) return transportFailure(res.status, 'transport.downloadFailed');
			const blob = await res.blob();
			if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged();
			return { code: 0, message: '', data: { blob, headers: res.headers } };
		} catch {
			if (!this.isAuthSessionEpoch(epoch)) return authSessionChanged();
			return transportFailure<{ blob: Blob; headers: Headers }>(500, 'transport.networkError');
		}
	}

	patch<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PATCH', endpoint, data);
	}

	put<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PUT', endpoint, data);
	}

	delete<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('DELETE', endpoint, data);
	}

	deleteWithHeaders<T>(endpoint: string, headers: Record<string, string>): Promise<ApiResult<T>> {
		return this.request<T>('DELETE', endpoint, undefined, true, headers);
	}
}

export const apiClient = new ApiClient();
