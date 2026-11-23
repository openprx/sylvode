import { error } from '@sveltejs/kit';

// A path no route matches: answered as a 404 inside the application shell (`../+error.svelte`).
export function load(): never {
	error(404, 'Not found');
}
