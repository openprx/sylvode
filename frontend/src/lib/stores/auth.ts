// 认证状态管理
import { derived, get, writable } from 'svelte/store';
import type { User } from '$lib/api/auth';
import { apiClient } from '$lib/api/client';
import { clearFlowLocalData } from '$lib/flow/local-data';

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
		/** Signs out locally and deletes every Flow IndexedDB database of this browser (outbox,
		 * recovery drafts, snapshot cache; `lib/flow/local-data.ts`). Resolves once they are gone. */
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
