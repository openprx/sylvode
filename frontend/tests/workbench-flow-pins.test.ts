import { writable } from 'svelte/store';
import { TaskWorkbench } from '../src/lib/workbench/tabs';
import { FlowTaskSessions } from '../src/lib/workbench/flow-sessions';
import type { FlowObjectRepository } from '../src/lib/flow/object-repository';
import { Suite, assertEqual, finish } from './support/harness';
const suite = new Suite('workbench Flow pins');
await suite.checkAsync(
	'page/navigation pins share resources and last consumer alone disposes',
	async () => {
		const state = new TaskWorkbench();
		state.activateUser('actor');
		state.visit('/flow/a');
		state.visit('/flow/b');
		const refs = new Map<string, number>();
		let connections = 0;
		let disposed = 0;
		const entries = new Map<
			string,
			{
				pending: ReturnType<typeof writable<number>>;
				docGeneration: ReturnType<typeof writable<number>>;
				doc: { subscribe: (listener: unknown) => () => void };
			}
		>();
		const projections = new Map<
			string,
			ReturnType<typeof writable<{ title: string; documentSeq: number }>>
		>();
		const repo = {
			open: async ({ objectId }: { objectId: string }) => {
				if (!entries.has(objectId)) {
					connections++;
					entries.set(objectId, {
						pending: writable(0),
						docGeneration: writable(0),
						doc: { subscribe: () => () => {} }
					});
					projections.set(objectId, writable({ title: 'Page', documentSeq: 0 }));
				}
				refs.set(objectId, (refs.get(objectId) ?? 0) + 1);
				return entries.get(objectId)!;
			},
			getProjection: (id: string) => projections.get(id)!,
			close: async (id: string) => {
				const remaining = (refs.get(id) ?? 0) - 1;
				refs.set(id, remaining);
				if (remaining === 0) {
					disposed++;
					entries.delete(id);
				}
			},
			disposeAll: async () => {}
		} as unknown as FlowObjectRepository;
		const host = new FlowTaskSessions('actor', state, repo);
		await host.retain('flow:a', 'navigator', 'scope', 'root');
		await host.retain('flow:a', 'navigator', 'scope', 'root');
		await host.retain('flow:b', 'navigator', 'scope', 'root');
		assertEqual(connections, 1, 'one shared navigation session');
		assertEqual(refs.get('root'), 2, 'one pin per task slot');
		state.close('flow:a', true);
		assertEqual(disposed, 0, 'another consumer retained');
		state.close('flow:b', true);
		assertEqual(disposed, 1, 'last consumer disposes');
		host.dispose();
		assertEqual(disposed, 1, 'repeat teardown does not double-close');
	}
);
finish(suite.result());
