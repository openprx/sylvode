/** Read-only existing semantic model. Never initializes or mutates a document. */
export interface LegacyProseBlock {
	id: string;
	parent: string | null;
	order: string;
	text: string;
	kind: string;
}
export function legacyProseBlocks(semantic: unknown): LegacyProseBlock[] {
	if (!semantic || typeof semantic !== 'object') return [];
	const nodes = (semantic as Record<string, unknown>).nodes;
	if (!nodes || typeof nodes !== 'object' || Array.isArray(nodes)) return [];
	return Object.entries(nodes)
		.flatMap(([id, raw]) => {
			if (!raw || typeof raw !== 'object') return [];
			const node = raw as Record<string, unknown>;
			if (node.deleted === true || node.kind !== 'block' || typeof node.text !== 'string')
				return [];
			const props =
				node.properties && typeof node.properties === 'object'
					? (node.properties as Record<string, unknown>)
					: {};
			if (
				['embed_object_id', 'embed_type', 'child_page_id', 'reference_id'].some(
					(key) => key in props
				)
			)
				return [];
			return [
				{
					id,
					parent: typeof node.parent === 'string' ? node.parent : null,
					order: typeof node.order_key === 'string' ? node.order_key : '',
					text: node.text,
					kind: typeof props.block_type === 'string' ? props.block_type : 'paragraph'
				}
			];
		})
		.sort((a, b) => a.order.localeCompare(b.order) || a.id.localeCompare(b.id));
}

export function canEnableLegacyEditor(object: import('../api/flow').FlowObjectView): boolean {
	return (
		object.object_type === 'page' &&
		object.editor_state === 'migration_required' &&
		object.editor_initialization_supported === true &&
		object.permitted_actions?.includes('edit') === true &&
		object.lifecycle_status === 'active'
	);
}
export function initializeEditorRequest(
	frontier: string,
	key: string
): import('../api/flow').ExecuteFlowCommandInput {
	return {
		command: { type: 'initialize_editor_doc', payload: {} },
		expected_frontier: frontier,
		idempotency_key: key
	};
}

export function editorMigrationState(
	error: import('./types').FlowError
): 'migration_required' | 'unsupported' | null {
	if (error.code !== 'unsupported_protocol' || !error.details || typeof error.details !== 'object')
		return null;
	const reason = (error.details as Record<string, unknown>).reason;
	return reason === 'editor_migration_required'
		? 'migration_required'
		: reason === 'editor_migration_unsupported'
			? 'unsupported'
			: null;
}
