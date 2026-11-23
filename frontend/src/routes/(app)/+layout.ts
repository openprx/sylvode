import { resolve } from '$app/paths';
// 应用路由布局（需要认证）
import { goto } from '$app/navigation';
import { browser } from '$app/env';
import { error } from '@sveltejs/kit';
import { apiClient } from '#lib/api/client.js';

export const ssr = false;

// The catch-all that answers unknown paths with a 404 (see `[...unmatched]/+page.ts`).
const UNMATCHED_ROUTE = '/(app)/[...unmatched]';

export function load({ route }: { route: { id: string | null } }) {
	if (browser) {
		const token = apiClient.getToken();
		if (!token) {
			// Signed out: an unknown path gets the standalone 404 page (`../+error.svelte`), every
			// other application path goes to the sign-in page.
			if (route.id === UNMATCHED_ROUTE) error(404, 'Not found');
			goto(resolve('/(auth)/auth/login'));
			return;
		}
	}

	return {};
}
