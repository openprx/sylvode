// 认证状态管理
import { derived, get, writable } from 'svelte/store';
import type { User } from '#lib/api/auth.js';
import { apiClient, type ApiResult } from '#lib/api/client.js';
import { clearFlowLocalData } from '#lib/flow/local-data.js';

interface AuthState {
	user: User | null;
	loading: boolean;
	error: string | null;
}

function createAuthStore() {
	// 从 localStorage 恢复 user
	let initialUser: User | null = null;
	if (typeof window !== 'undefined') {
		const savedUser = localStorage.getItem('auth_user');
		if (savedUser) {
			try {
				initialUser = JSON.parse(savedUser);
			} catch (e) {
				console.error('Failed to parse saved user', e);
			}
		}
	}

	const { subscribe, set, update } = writable<AuthState>({
		user: initialUser,
		loading: false,
		error: null
	});

	return {
		subscribe,
		setUser: (user: User | null) => {
			update((state) => ({ ...state, user, loading: false }));
			// 持久化到 localStorage
			if (typeof window !== 'undefined') {
				if (user) {
					localStorage.setItem('auth_user', JSON.stringify(user));
				} else {
					localStorage.removeItem('auth_user');
				}
			}
		},
		setLoading: (loading: boolean) => {
			update((state) => ({ ...state, loading }));
		},
		setError: (error: string | null) => {
			update((state) => ({ ...state, error }));
		},
		/** Ends the local session only (expired or unverifiable session, network failure): the
		 * user's own Flow IndexedDB databases stay, so their unsent intents replay on their next
		 * sign-in. They are per user (`lib/flow/local-data.ts`), so another account never sees them. */
		endSession: (): void => {
			apiClient.clearAuth();
			if (typeof window !== 'undefined') {
				localStorage.removeItem('auth_user');
			}
			set({ user: null, loading: false, error: null });
		},
		/** The user signed out: ends the session and deletes every Flow IndexedDB database of this
		 * browser (outbox, recovery drafts, snapshot cache). Resolves once they are gone (bounded,
		 * see `clearFlowLocalData`). */
		logout: (): Promise<void> => {
			const userId = get({ subscribe })?.user?.id;
			apiClient.clearAuth();
			if (typeof window !== 'undefined') {
				localStorage.removeItem('auth_user');
			}
			set({ user: null, loading: false, error: null });
			return clearFlowLocalData(userId ? [userId] : []);
		},
		reset: () => {
			set({ user: null, loading: true, error: null });
		}
	};
}

export const authStore = createAuthStore();
export const isAuthenticated = derived(authStore, ($auth) => $auth.user !== null);

/** The app shell's session check: `me` answers who is signed in. Any failure (an invalid or
 * expired session, a network failure, a 5xx) only ends the local session -- it is not the user
 * signing out, so local Flow data is kept. */
export async function verifySession(
	me: () => Promise<ApiResult<{ user: User }>>
): Promise<'signed_in' | 'ended'> {
	const response = await me();
	if (response.code !== 0) {
		authStore.endSession();
		return 'ended';
	}
	if (response.data) authStore.setUser(response.data.user);
	return 'signed_in';
}
