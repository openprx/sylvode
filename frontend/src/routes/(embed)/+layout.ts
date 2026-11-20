// Embedded surfaces: the same session check as the app routes, no shell.
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { browser } from '$app/env';
import { apiClient } from '#lib/api/client.js';

export const ssr = false;

export function load() {
	if (browser) {
		const token = apiClient.getToken();
		if (!token) {
			goto(resolve('/(auth)/auth/login'));
			return;
		}
	}

	return {};
}
