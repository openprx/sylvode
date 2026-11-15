import { test } from 'node:test';
import assert from 'node:assert/strict';
import { conversionAvailability } from '../src/lib/flow/convert-availability';
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
