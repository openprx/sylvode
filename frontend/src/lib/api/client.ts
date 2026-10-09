const API_BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '';

export interface ApiResult<T> {
	code: number;
	message: string;
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
		message: typeof parsed.message === 'string' ? parsed.message : 'Invalid response format',
		data: (parsed.data as T | null) ?? null,
		...(typeof parsed.error_code === 'string' ? { error_code: parsed.error_code } : {}),
		...(parsed.details === undefined ? {} : { details: parsed.details })
	};
}

class ApiClient {
	private baseUrl: string;
	private token: string | null = null;
	private refreshTokenValue: string | null = null;
	private refreshInFlight: Promise<boolean> | null = null;

	constructor(baseUrl: string = API_BASE_URL) {
		this.baseUrl = baseUrl;
		if (typeof window !== 'undefined') {
			this.token = localStorage.getItem('auth_token');
			this.refreshTokenValue = localStorage.getItem('refresh_token');
		}
	}

	setToken(token: string | null) {
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
		this.setToken(null);
		this.setRefreshToken(null);
	}

	private redirectToLogin() {
		if (typeof window !== 'undefined') {
			window.location.href = '/auth/login';
		}
	}

	private async performTokenRefresh(): Promise<boolean> {
		const refreshToken = this.getRefreshToken();
		const body = refreshToken ? JSON.stringify({ refresh_token: refreshToken }) : undefined;
		const url = `${this.baseUrl}/api/v1/auth/refresh`;
		const headers = new Headers();
		headers.set('Content-Type', 'application/json');

		try {
			const res = await fetch(url, {
				method: 'POST',
				headers,
				body,
				credentials: 'include'
			});
			const parsed = (await res.json()) as Partial<
				ApiResult<{
					tokens?: { access_token?: string; refresh_token?: string };
				}>
			>;

			if (
				parsed.code === 0 &&
				parsed.data?.tokens?.access_token &&
				typeof parsed.data.tokens.access_token === 'string'
			) {
				this.setToken(parsed.data.tokens.access_token);
				if (typeof parsed.data.tokens.refresh_token === 'string') {
					this.setRefreshToken(parsed.data.tokens.refresh_token);
				}
				return true;
			}
		} catch (_error) {
			return false;
		}

		return false;
	}

	private async refreshAccessTokenOnce(): Promise<boolean> {
		if (!this.refreshInFlight) {
			this.refreshInFlight = this.performTokenRefresh().finally(() => {
				this.refreshInFlight = null;
			});
		}
		return this.refreshInFlight;
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
	async ensureFreshAccessToken(): Promise<boolean> {
		return this.refreshAccessTokenOnce();
	}

	async request<T>(
		method: string,
		endpoint: string,
		body?: unknown,
		retryAfterRefresh: boolean = true,
		additionalHeaders: Record<string, string> = {},
		signal?: AbortSignal
	): Promise<ApiResult<T>> {
		const url = `${this.baseUrl}${endpoint}`;
		const headers = new Headers();
		const isFormData = typeof FormData !== 'undefined' && body instanceof FormData;
		if (!isFormData) headers.set('Content-Type', 'application/json');
		if (this.token) {
			headers.set('Authorization', `Bearer ${this.token}`);
		}
		for (const [name, value] of Object.entries(additionalHeaders)) headers.set(name, value);

		try {
			const res = await fetch(url, {
				method,
				headers,
				body: isFormData ? body : body ? JSON.stringify(body) : undefined,
				signal,
				credentials: 'include'
			});

			const parsed = (await res.json()) as Partial<ApiResult<T>>;
			const result = normalizeEnvelope<T>(parsed);

			if (result.code === 401) {
				const isRefreshEndpoint = endpoint === '/api/v1/auth/refresh';
				if (!isRefreshEndpoint && retryAfterRefresh) {
					const refreshed = await this.refreshAccessTokenOnce();
					if (refreshed) {
						return this.request<T>(method, endpoint, body, false, additionalHeaders, signal);
					}
				}
				this.clearAuth();
				this.redirectToLogin();
			}

			return result;
		} catch (error) {
			return {
				code: 500,
				message: error instanceof Error ? error.message : 'Network error',
				data: null
			};
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
		const result = await new Promise<ApiResult<T>>((resolve) => {
			const failed = (message: string): ApiResult<T> => ({ code: 500, message, data: null });
			if (signal?.aborted) {
				resolve(failed('Request aborted'));
				return;
			}
			const xhr = new XMLHttpRequest();
			xhr.open('POST', `${this.baseUrl}${endpoint}`);
			xhr.withCredentials = true;
			if (this.token) xhr.setRequestHeader('Authorization', `Bearer ${this.token}`);
			for (const [name, value] of Object.entries(headers)) xhr.setRequestHeader(name, value);
			xhr.upload.onprogress = (event) => {
				onProgress({ loaded: event.loaded, total: event.lengthComputable ? event.total : null });
			};
			const onAbort = () => xhr.abort();
			signal?.addEventListener('abort', onAbort, { once: true });
			const settle = (value: ApiResult<T>) => {
				signal?.removeEventListener('abort', onAbort);
				resolve(value);
			};
			xhr.onload = () => {
				try {
					settle(normalizeEnvelope<T>(JSON.parse(xhr.responseText) as Partial<ApiResult<T>>));
				} catch {
					settle(failed('Invalid response format'));
				}
			};
			xhr.onerror = () => settle(failed('Network error'));
			xhr.onabort = () => settle(failed('Request aborted'));
			xhr.send(data);
		});
		if (result.code === 401 && retryAfterRefresh) {
			if (await this.refreshAccessTokenOnce()) {
				return this.xhrFormData<T>(endpoint, data, headers, signal, onProgress, false);
			}
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
	async getBinary(
		endpoint: string,
		retryAfterRefresh: boolean = true
	): Promise<ApiResult<{ blob: Blob; headers: Headers }>> {
		const headers = new Headers();
		if (this.token) headers.set('Authorization', `Bearer ${this.token}`);
		try {
			const res = await fetch(`${this.baseUrl}${endpoint}`, {
				method: 'GET',
				headers,
				credentials: 'include'
			});
			const contentType = res.headers.get('Content-Type') ?? '';
			if (contentType.startsWith('application/json')) {
				const result = normalizeEnvelope<{ blob: Blob; headers: Headers }>(
					(await res.json()) as Partial<ApiResult<{ blob: Blob; headers: Headers }>>
				);
				if (result.code === 401) {
					if (retryAfterRefresh && (await this.refreshAccessTokenOnce())) {
						return this.getBinary(endpoint, false);
					}
					this.clearAuth();
					this.redirectToLogin();
				}
				return { ...result, data: null };
			}
			if (!res.ok) return { code: res.status, message: 'Download failed', data: null };
			return { code: 0, message: 'ok', data: { blob: await res.blob(), headers: res.headers } };
		} catch (error) {
			return {
				code: 500,
				message: error instanceof Error ? error.message : 'Network error',
				data: null
			};
		}
	}

	patch<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PATCH', endpoint, data);
	}

	put<T>(endpoint: string, data?: unknown): Promise<ApiResult<T>> {
		return this.request<T>('PUT', endpoint, data);
	}

	delete<T>(endpoint: string): Promise<ApiResult<T>> {
		return this.request<T>('DELETE', endpoint);
	}

	deleteWithHeaders<T>(endpoint: string, headers: Record<string, string>): Promise<ApiResult<T>> {
		return this.request<T>('DELETE', endpoint, undefined, true, headers);
	}
}

export const apiClient = new ApiClient();
