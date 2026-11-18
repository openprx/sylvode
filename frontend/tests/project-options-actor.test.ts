import { test } from 'node:test';
import assert from 'node:assert/strict';
import { get } from 'svelte/store';
import { createProjectOptionsStore } from '../src/lib/stores/project-options';
import { workspacesApi } from '../src/lib/api/workspaces';
import { projectsApi } from '../src/lib/api/projects';
function deferred<T>() {
	let resolve!: (value: T) => void;
	return { promise: new Promise<T>((done) => (resolve = done)), resolve };
}
test('late actor A project results cannot populate actor B navigation; same-actor selection preserves loaded data', async () => {
	const oldWorkspaces = workspacesApi.list,
		oldProjects = projectsApi.list,
		late = deferred<Awaited<ReturnType<typeof projectsApi.list>>>();
	let actor = 'A';
	workspacesApi.list = async () => ({
		code: 0,
		message: 'ok',
		data: {
			items: [{ id: actor, slug: actor, name: actor, created_at: '', updated_at: '' }],
			total: 1,
			page: 1,
			per_page: 100,
			total_pages: 1
		}
	});
	projectsApi.list = async (workspace) =>
		workspace === 'A'
			? late.promise
			: {
					code: 0,
					message: 'ok',
					data: {
						items: [
							{
								id: 'B-project',
								workspace_id: 'B',
								name: 'B project',
								key: 'B',
								type_key: 'general',
								type_settings: {},
								created_at: '',
								updated_at: ''
							}
						],
						total: 1,
						page: 1,
						per_page: 100,
						total_pages: 1
					}
				};
	const store = createProjectOptionsStore();
	try {
		store.selectUser('actor-A');
		const a = store.ensureLoaded();
		await Promise.resolve();
		await Promise.resolve();
		actor = 'B';
		store.selectUser('actor-B');
		const b = await store.ensureLoaded();
		assert.deepEqual(
			b.map((item) => item.id),
			['B-project']
		);
		late.resolve({
			code: 0,
			message: 'ok',
			data: {
				items: [
					{
						id: 'A-private-project',
						workspace_id: 'A',
						name: 'A private project',
						key: 'A',
						type_key: 'general',
						type_settings: {},
						created_at: '',
						updated_at: ''
					}
				],
				total: 1,
				page: 1,
				per_page: 100,
				total_pages: 1
			}
		});
		assert.deepEqual(await a, []);
		assert.deepEqual(
			get(store).items.map((item) => item.id),
			['B-project']
		);
		store.selectUser('actor-B');
		assert.equal(get(store).loaded, true);
		assert.deepEqual(
			get(store).items.map((item) => item.id),
			['B-project']
		);
	} finally {
		workspacesApi.list = oldWorkspaces;
		projectsApi.list = oldProjects;
	}
});
