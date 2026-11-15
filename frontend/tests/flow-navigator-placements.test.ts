import { navigatorPlacements } from '../src/lib/flow/navigator-placements';
import type { FlowNavigatorNode } from '../src/lib/api/flow';
import { Suite, assert, assertEqual, finish } from './support/harness';
const suite = new Suite('flow_navigator_placements');
const node: FlowNavigatorNode = {
	object_id: 'source-object',
	scope_id: 'source-scope',
	mount_id: 'mount-1',
	ownership: 'external',
	parent_id: 'destination-root',
	position: 'mount:mount-1',
	title: 'Source page',
	type: 'page'
};
suite.check(
	'external placement keeps source identity and never substitutes destination ownership',
	() => {
		const [item] = navigatorPlacements([node]);
		assertEqual(item.scopeId, 'source-scope', 'source scope');
		assertEqual(item.objectId, 'source-object', 'canonical source object');
		assertEqual(item.mountId, 'mount-1', 'independent placement');
		assert(!('parentId' in item), 'source ancestry is not inferred from destination placement');
	}
);
suite.check(
	'multiple placements of one source remain independent and canonical nodes stay outside the placement list',
	() => {
		const items = navigatorPlacements([
			node,
			{ ...node, mount_id: 'mount-2' },
			{ ...node, mount_id: undefined, ownership: 'owned' }
		]);
		assertEqual(items.length, 2, 'one entry per mount, not per source');
	}
);
suite.check(
	'team context cannot manufacture removal rights; only explicit resource capability enables it',
	() => {
		assertEqual(navigatorPlacements([node])[0].canRemove, false, 'no role-based inference');
		assertEqual(
			navigatorPlacements([{ ...node, can_remove: true }])[0].canRemove,
			true,
			'explicit capability'
		);
		assertEqual(
			navigatorPlacements([{ ...node, mount_status: 'source_archived' }])[0].status,
			'source_archived',
			'source archive differs from placement removal'
		);
	}
);
for (const invalid of [
	{ ...node, scope_id: undefined },
	{ ...node, mount_id: undefined },
	{ ...node, scope_id: 42 as unknown as string }
])
	suite.check(
		'malformed placement identity fails closed: ' +
			String(invalid.scope_id) +
			'/' +
			String(invalid.mount_id),
		() => {
			let rejected = false;
			try {
				navigatorPlacements([invalid]);
			} catch (error) {
				rejected = (error as { code: string }).code === 'unsupported_protocol';
			}
			assert(rejected, 'no fabricated canonical resource');
		}
	);
suite.check('one placement ID cannot silently change its source scope or object', () => {
	let rejected = false;
	try {
		navigatorPlacements([node, { ...node, scope_id: 'other-scope' }]);
	} catch {
		rejected = true;
	}
	assert(rejected, 'conflicting identity rejected');
});
finish(suite.result());
