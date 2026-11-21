import { describe, expect, test } from 'bun:test';
import { LoroDoc } from 'loro-crdt';

// Hostile-update probe: a change creating a tree node under a parent that does not exist.
// Records exactly what a (subscribed) client doc does when it imports such an update.

// JSON updates use compressed ids `counter@peerIndex`; the peer table gets one extra, unknown peer.
const GHOST_PEER = '4242';

type Outcome = { importError: string | null; events: number; afterError: string | null; nodes: number };

function describeError(e: unknown): string {
	if (e instanceof Error) return `${e.constructor.name}: ${e.message}`;
	return String(e);
}

// Build an honest doc with a small tree, then craft the hostile update on `attacker`.
function setup(attackerSamePeer: boolean) {
	const honest = new LoroDoc();
	honest.setPeerId(1);
	const t = honest.getTree('tree');
	const root = t.createNode();
	root.data.set('k', 'root');
	honest.commit();
	const base = honest.export({ mode: 'snapshot' });
	const baseVv = honest.version();

	const attacker = new LoroDoc();
	attacker.setPeerId(attackerSamePeer ? 1 : 2);
	attacker.import(base);
	const before = attacker.version();
	attacker.getTree('tree').createNode(root.id); // valid create; its parent is rewritten below
	attacker.commit();
	// Rewrite the op's parent to an id that exists nowhere. The change still depends exactly on the head.
	const json: any = attacker.exportJsonUpdates(baseVv);
	const ghostIndex = json.peers.push(GHOST_PEER) - 1;
	let rewritten = 0;
	for (const change of json.changes) {
		for (const op of change.ops as Array<{ content: { type: string; parent?: string } }>) {
			if (op.content.type === 'create' && op.content.parent !== undefined) {
				op.content.parent = `999999@${ghostIndex}`;
				rewritten += 1;
			}
		}
	}
	if (rewritten !== 1) throw new Error(`expected one rewritten op, got ${rewritten}: ${JSON.stringify(json)}`);
	const forge = new LoroDoc();
	forge.setPeerId(attackerSamePeer ? 1 : 2);
	forge.import(base);
	forge.importJsonUpdates(json);
	const hostile = forge.export({ mode: 'update', from: baseVv });
	void before;
	return { honest, base, hostile };
}

function run(attackerSamePeer: boolean, subscribe: boolean): Outcome {
	const { base, hostile } = setup(attackerSamePeer);
	const victim = new LoroDoc();
	victim.setPeerId(3);
	victim.import(base);
	let events = 0;
	if (subscribe) victim.subscribe(() => { events += 1; });
	let importError: string | null = null;
	try {
		victim.import(hostile);
	} catch (e) {
		importError = describeError(e);
	}
	let afterError: string | null = null;
	let nodes = -1;
	try {
		nodes = victim.getTree('tree').nodes().length;
		JSON.stringify(victim.toJSON());
		victim.getTree('tree').toArray();
		victim.getTree('tree').getNodes({ withDeleted: true });
		victim.getTree('tree').getNodes({ withDeleted: false });
		victim.export({ mode: 'snapshot' });
		victim.commit();
	} catch (e) {
		afterError = describeError(e);
	}
	return { importError, events, afterError, nodes };
}

describe('dangling-parent import', () => {
	for (const same of [true, false]) {
		for (const sub of [true, false]) {
			test(`samePeer=${same} subscribed=${sub}`, () => {
				const o = run(same, sub);
				console.log(`RESULT samePeer=${same} subscribed=${sub} ${JSON.stringify(o)}`);
				expect(o).toBeDefined();
			});
		}
	}
});

// Follow-up scenarios: what a client that did NOT crash on the live update does next.
describe('dangling-parent: snapshot and bootstrap paths', () => {
	function serverAfter(): { base: Uint8Array; snapshot: Uint8Array } {
		const { base, hostile } = setup(true);
		const server = new LoroDoc(); // the server engine never subscribes
		server.setPeerId(99);
		server.import(base);
		server.import(hostile);
		return { base, snapshot: server.export({ mode: 'snapshot' }) };
	}
	function attempt(label: string, f: () => unknown) {
		let r: string;
		try {
			r = `ok ${JSON.stringify(f())}`;
		} catch (e) {
			r = `THROWS ${describeError(e)}`;
		}
		console.log(`RESULT2 ${label}: ${r}`);
	}

	test('bootstrap into a fresh doc, subscribe afterwards, then edit', () => {
		const { snapshot } = serverAfter();
		const doc = new LoroDoc();
		doc.setPeerId(7);
		attempt('fresh import of server snapshot', () => doc.import(snapshot));
		let events = 0;
		doc.subscribe(() => { events += 1; });
		attempt('toJSON', () => Object.keys(doc.toJSON()));
		attempt('nodes()', () => doc.getTree('tree').nodes().length);
		attempt('local create + commit', () => {
			const root = doc.getTree('tree').roots()[0];
			root.createNode();
			doc.commit();
			return events;
		});
		attempt('export snapshot', () => doc.export({ mode: 'snapshot' }).length);
	});

	test('server snapshot imported into an already subscribed doc (reconnect path)', () => {
		const { base, snapshot } = serverAfter();
		const doc = new LoroDoc();
		doc.setPeerId(7);
		doc.import(base);
		let events = 0;
		doc.subscribe(() => { events += 1; });
		attempt('import server snapshot into subscribed doc', () => doc.import(snapshot));
		attempt('then toJSON', () => Object.keys(doc.toJSON()));
		attempt('events', () => events);
	});

	test('after a panic the same doc stays dead (re-import snapshot, local edit)', () => {
		const { base, hostile } = setup(true);
		const doc = new LoroDoc();
		doc.setPeerId(7);
		doc.import(base);
		doc.subscribe(() => {});
		attempt('hostile', () => doc.import(hostile));
		attempt('re-import base snapshot', () => doc.import(base));
		attempt('local edit', () => {
			doc.getTree('tree').createNode();
			doc.commit();
		});
		attempt('export', () => doc.export({ mode: 'snapshot' }).length);
	});
});
