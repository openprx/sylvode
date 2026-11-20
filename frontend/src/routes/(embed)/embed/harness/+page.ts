// The harness is an admin test surface: guarded like the admin routes.
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { browser } from '$app/env';
import { isAdminUser } from '#lib/utils/auth.js';
import type { User } from '#lib/api/auth.js';

export function load() {
	if (!browser) return {};

	const raw = localStorage.getItem('auth_user');
	if (!raw) {
		goto(resolve('/(app)/workspace'));
		return {};
	}

	try {
		const user = JSON.parse(raw) as User;
		if (!isAdminUser(user)) goto(resolve('/(app)/workspace'));
	} catch {
		goto(resolve('/(app)/workspace'));
	}

	return {};
}
