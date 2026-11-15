import { LoroDoc } from 'loro-crdt';
import { writable } from 'svelte/store';
import { observeNavigatorDocument } from '../src/lib/flow/navigator-doc-observer';
import { Suite, assertEqual, finish } from './support/harness';
const suite = new Suite('navigator document observer');
await suite.checkAsync(
	'accepted remote map edits refresh SQL metadata and generation replacement detaches old document',
	async () => {
		let current = new LoroDoc();
		current.setPeerId('6101');
		const generation = writable(0);
		let changes = 0;
		let remote = 0;
		const stop = observeNavigatorDocument(
			{
				get doc() {
					return current;
				},
				docGeneration: generation
			},
			() => {
				changes++;
			},
			() => {
				remote++;
			}
		);
		current.getMap('order').set('object', 'i');
		current.commit();
		await Promise.resolve();
		assertEqual(remote, 0, 'local order writes do not refetch SQL');
		const peer = current.fork();
		peer.setPeerId('6102');
		peer.getMap('order').set('object', 'r');
		peer.commit();
		current.import(peer.export({ mode: 'update', from: current.version() }));
		await Promise.resolve();
		assertEqual(remote, 1, 'accepted remote order update refreshes SQL hierarchy');
		const old = current;
		current = peer.fork();
		generation.set(1);
		const bound = changes;
		old.getMap('order').set('object', 'v');
		old.commit();
		await Promise.resolve();
		assertEqual(changes, bound, 'old document is detached');
		assertEqual(remote, 2, 'recovery replacement refreshes metadata once');
		stop();
		current.getMap('order').set('object', 'z');
		current.commit();
		await Promise.resolve();
		assertEqual(changes, bound, 'cleanup detaches current document');
	}
);
finish(suite.result());
