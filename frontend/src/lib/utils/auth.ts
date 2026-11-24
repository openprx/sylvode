import type { User } from '#lib/api/auth.js';

export function isAdminUser(user: User | null): boolean {
	if (!user) {
		return false;
	}

	return user.role === 'admin' || user.role === 'owner';
}
