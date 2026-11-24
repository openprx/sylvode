import { apiClient, authSessionChanged, type ApiResult } from './client';
import { transportFailure } from './transport';

export interface LoginRequest {
	email: string;
	password: string;
}

export interface User {
	id: string;
	email: string;
	name: string;
	role?: 'owner' | 'admin' | 'member';
	status?: 'active' | 'disabled';
	avatar_url?: string;
	created_at: string;
	updated_at?: string;
}

export interface RegisterRequest {
	email: string;
	password: string;
	name: string;
}

export interface LoginResponse {
	tokens: {
		access_token: string;
		refresh_token: string;
		token_type: string;
		access_expires_in: number;
		refresh_expires_in: number;
	};
	user: User;
}

export interface UpdateProfileRequest {
	name: string;
	email: string;
	avatar_url?: string | null;
}

export interface UpdatePasswordRequest {
	current_password: string;
	new_password: string;
}

export interface NotificationPreferences {
	email_notification: boolean;
	mention_only: boolean;
	daily_digest: boolean;
}

export const authApi = {
	async login(credentials: LoginRequest): Promise<ApiResult<LoginResponse>> {
		const epoch = apiClient.beginAuthSession();
		const result = await apiClient.request<LoginResponse>(
			'POST',
			'/api/v1/auth/login',
			credentials,
			false
		);
		if (!apiClient.isAuthSessionEpoch(epoch))
			return apiClient.bindAuthResult(authSessionChanged<LoginResponse>(), epoch);
		if (result.code === 0 && result.data?.tokens?.access_token)
			apiClient.installAuthSession(
				result.data.tokens.access_token,
				result.data.tokens.refresh_token ?? null,
				result.data.user.id
			);
		return apiClient.bindAuthResult(result);
	},

	register(data: RegisterRequest): Promise<ApiResult<{ user: User }>> {
		return apiClient.post<{ user: User }>('/api/v1/auth/register', data);
	},

	me(): Promise<ApiResult<{ user: User }>> {
		return apiClient.get<{ user: User }>('/api/v1/auth/me');
	},

	updateProfile(data: UpdateProfileRequest): Promise<ApiResult<{ user: User }>> {
		return apiClient.put<{ user: User }>('/api/v1/auth/me/profile', data);
	},

	updatePassword(data: UpdatePasswordRequest): Promise<ApiResult<null>> {
		return apiClient.put<null>('/api/v1/auth/me/password', data);
	},

	getPreferences(): Promise<ApiResult<{ notification_prefs: NotificationPreferences }>> {
		return apiClient.get<{ notification_prefs: NotificationPreferences }>(
			'/api/v1/auth/me/preferences'
		);
	},

	updatePreferences(
		notification_prefs: NotificationPreferences
	): Promise<ApiResult<{ notification_prefs: NotificationPreferences }>> {
		return apiClient.put<{ notification_prefs: NotificationPreferences }>(
			'/api/v1/auth/me/preferences',
			{
				notification_prefs
			}
		);
	},

	async logout(): Promise<ApiResult<null>> {
		const epoch = apiClient.beginAuthSession();
		const result = await apiClient.request<null>('POST', '/api/v1/auth/logout', undefined, false);
		if (!apiClient.isAuthSessionEpoch(epoch))
			return apiClient.bindAuthResult(authSessionChanged<null>(), epoch);
		apiClient.clearAuth();
		return apiClient.bindAuthResult(result);
	},
	async refreshToken(): Promise<ApiResult<{ token: string }>> {
		const epoch = apiClient.getAuthSessionEpoch();
		const refreshed = await apiClient.ensureFreshAccessToken(epoch);
		if (!apiClient.isAuthSessionEpoch(epoch)) return authSessionChanged();
		const token = apiClient.getToken();
		return refreshed && token
			? { code: 0, message: '', data: { token } }
			: transportFailure(401, 'transport.sessionRefreshFailed');
	}
};
