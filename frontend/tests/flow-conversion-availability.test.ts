import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	conversionAvailability,
	defaultConversionDestination
} from '../src/lib/flow/convert-availability';
const page = {
	object_type: 'page' as const,
	lifecycle_status: 'active',
	permitted_actions: ['edit']
};
test('personal Forms conversion is unavailable unless the actual source explicitly advertises support', () => {
	assert.deepEqual(conversionAvailability(page), { mode: 'unavailable' });
	assert.deepEqual(
		conversionAvailability({
			...page,
			supported_forms_conversion: false,
			forms_conversion_destination_mode: 'unavailable'
		}),
		{ mode: 'unavailable' }
	);
	assert.deepEqual(
		conversionAvailability({
			...page,
			supported_forms_conversion: true,
			forms_conversion_destination_mode: 'explicit_workspace'
		}),
		{ mode: 'explicit_workspace' }
	);
});
test('legacy destination comes from the actual source binding; missing personal fields never create a workspace context', () => {
	assert.deepEqual(conversionAvailability({ ...page, workspace_id: 'actual-source-workspace' }), {
		mode: 'same_workspace',
		workspaceId: 'actual-source-workspace'
	});
	assert.deepEqual(
		conversionAvailability({
			...page,
			workspace_id: 'actual-source-workspace',
			supported_forms_conversion: true,
			forms_conversion_destination_mode: 'same_workspace'
		}),
		{ mode: 'same_workspace', workspaceId: 'actual-source-workspace' }
	);
	assert.deepEqual(
		conversionAvailability({
			...page,
			supported_forms_conversion: true,
			forms_conversion_destination_mode: 'same_workspace'
		}),
		{ mode: 'unavailable' }
	);
	assert.deepEqual(
		conversionAvailability({
			...page,
			workspace_id: 'actual-source-workspace',
			supported_forms_conversion: false
		}),
		{ mode: 'unavailable' }
	);
});
test('inactive/non-Page/read-only sources cannot expose a conversion workflow', () => {
	for (const patch of [
		{ lifecycle_status: 'archived' },
		{ object_type: 'collection' as const },
		{ permitted_actions: ['view'] }
	])
		assert.deepEqual(
			conversionAvailability({
				...page,
				...patch,
				workspace_id: 'actual-source-workspace',
				supported_forms_conversion: true,
				forms_conversion_destination_mode: 'same_workspace'
			}),
			{ mode: 'unavailable' }
		);
});

test('an explicit destination starts with the source workspace only when the user may pick it', () => {
	const offered = [{ id: 'source-ws' }, { id: 'other-ws' }];
	const explicit = { mode: 'explicit_workspace' } as const;
	assert.equal(defaultConversionDestination(explicit, 'source-ws', offered), 'source-ws');
	assert.equal(defaultConversionDestination(explicit, 'foreign-ws', offered), '');
	assert.equal(defaultConversionDestination(explicit, 'source-ws', []), '');
});
test('a personal source starts with no destination and an unavailable conversion with none either', () => {
	const offered = [{ id: 'source-ws' }];
	assert.equal(defaultConversionDestination({ mode: 'explicit_workspace' }, null, offered), '');
	assert.equal(
		defaultConversionDestination({ mode: 'explicit_workspace' }, undefined, offered),
		''
	);
	assert.equal(defaultConversionDestination({ mode: 'unavailable' }, 'source-ws', offered), '');
});
test('a same-workspace conversion stays fixed to the source workspace', () => {
	const same = { mode: 'same_workspace', workspaceId: 'source-ws' } as const;
	assert.equal(defaultConversionDestination(same, 'source-ws', [{ id: 'source-ws' }]), 'source-ws');
	assert.equal(defaultConversionDestination(same, 'source-ws', []), '');
});
