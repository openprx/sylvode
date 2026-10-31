// 应用路由布局（需要认证）
import { goto } from '$app/navigation';
import { browser } from '$app/env';
import { apiClient } from '#lib/api/client.js';

export const ssr = false;

export function load() {
	if (browser) {
		const token = apiClient.getToken();
		if (!token) {
			goto('/auth/login');
			return;
		}
	}

	return {};
}
