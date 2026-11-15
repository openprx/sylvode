import { goto } from '$app/navigation';
import { taskResource } from './tabs';
/** Cached values are validated same-app URL paths, not SvelteKit route-ID templates. */
export function navigateTask(path: string, replace = false): Promise<void> {
	const url = new URL(path, window.location.origin);
	if (
		url.origin !== window.location.origin ||
		(!taskResource(url.href) && url.pathname !== '/workspace')
	)
		return Promise.reject(new Error('Unsupported task route'));
	return goto(url, { reset: false, replace });
}
