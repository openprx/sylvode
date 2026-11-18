/** Read-only existing semantic model. Never initializes or mutates a document. */
export interface LegacyProseBlock {
	id: string;
	parent: string | null;
	order: string;
	text: string;
	kind: string;
	attrs: Record<string, string>;
	inline_spans: import('./inline-runs').PageInlineSpan[];
	indent: number;
}
export function legacyProseBlocks(semantic: unknown): LegacyProseBlock[] {
	if (!semantic || typeof semantic !== 'object') return [];
	const nodes = (semantic as Record<string, unknown>).nodes;
	if (!nodes || typeof nodes !== 'object' || Array.isArray(nodes)) return [];
	const records = Object.entries(nodes)
		.flatMap(([id, raw]) => {
			if (!raw || typeof raw !== 'object') return [];
			const node = raw as Record<string, unknown>;
			if (node.deleted === true || node.kind !== 'block') return [];
			const props =
				node.properties && typeof node.properties === 'object'
					? (node.properties as Record<string, unknown>)
					: {};
			if (['embed_object_id', 'embed_type', 'reference_id'].some((key) => key in props)) return [];
			const kind = typeof props.block_type === 'string' ? props.block_type : 'paragraph';
			if ('child_page_id' in props && kind !== 'childPage') return [];
			const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
			if (['image', 'attachment'].includes(kind) && !uuid.test(String(props.asset_id ?? '')))
				return [];
			if (kind === 'childPage' && !uuid.test(String(props.child_page_id ?? ''))) return [];
			if (typeof node.text !== 'string' && !['image', 'attachment', 'childPage'].includes(kind))
				return [];
			let spans: import('./inline-runs').PageInlineSpan[] = [];
			try {
				const value =
					typeof props.inline_spans === 'string'
						? JSON.parse(props.inline_spans)
						: props.inline_spans;
				if (Array.isArray(value)) spans = value;
			} catch {
				/* Unreadable marks do not become HTML. */
			}
			return [
				{
					id,
					parent: typeof node.parent === 'string' ? node.parent : null,
					order: typeof node.order_key === 'string' ? node.order_key : '',
					text: typeof node.text === 'string' ? node.text : '',
					attrs: Object.fromEntries(
						Object.entries(props).filter(
							(entry): entry is [string, string] => typeof entry[1] === 'string'
						)
					),
					inline_spans: spans,
					indent: Math.max(0, Math.min(7, Number(props.indent) || 0)),
					kind: typeof props.block_type === 'string' ? props.block_type : 'paragraph'
				}
			];
		})
		.sort((a, b) => a.order.localeCompare(b.order) || a.id.localeCompare(b.id));
	const ids = new Set(records.map((block) => block.id));
	const children = new Map<string | null, LegacyProseBlock[]>();
	for (const block of records) {
		const parent = block.parent && ids.has(block.parent) ? block.parent : null;
		const list = children.get(parent) ?? [];
		list.push(block);
		children.set(parent, list);
	}
	const output: LegacyProseBlock[] = [];
	const seen = new Set<string>();
	const walk = (parent: string | null, depth: number) => {
		for (const block of children.get(parent) ?? []) {
			if (seen.has(block.id)) continue;
			seen.add(block.id);
			output.push({ ...block, indent: parent === null ? block.indent : Math.min(7, depth) });
			walk(block.id, depth + 1);
		}
	};
	walk(null, 0);
	return output;
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
