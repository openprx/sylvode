// Renderer registry (`contracts/ui-surface-v1.md` "Renderer registry"): avoids a route-page type
// switch that grows every version. v0.4 registers `page:canvas` and `navigator:tree` plus the
// four block-level renderers this alpha ships. An unregistered `${objectType}:${viewType}` fails
// to `read_only`/`unsupported` -- it must never let an arbitrary component path reach the CRDT.

// `collection` and `record` are real object types from v0.6 (`FlowReadableObjectType`); until
// their `collection:table` / `collection:board` / `record:panel` renderers are registered every
// lookup for them returns `null`, i.e. the read_only/unsupported fallback.
export type ObjectType = 'page' | 'navigator' | 'collection' | 'record';
export type ViewType = 'canvas' | 'tree';
export type RendererKey = `${ObjectType}:${ViewType}`;
export type Capability = 'edit' | 'view';

export interface ViewRendererRegistration {
	readonly key: RendererKey;
	readonly capabilities: readonly Capability[];
	readonly fallback: 'unsupported' | 'read_only';
}

const REGISTRY: Partial<Record<RendererKey, ViewRendererRegistration>> = {
	'page:canvas': { key: 'page:canvas', capabilities: ['edit', 'view'], fallback: 'read_only' },
	'navigator:tree': { key: 'navigator:tree', capabilities: ['edit', 'view'], fallback: 'read_only' }
};

export function resolveRenderer(objectType: ObjectType, viewType: ViewType): ViewRendererRegistration | null {
	return REGISTRY[`${objectType}:${viewType}`] ?? null;
}

/** Narrows a wire `object_type` to a registry object type (`null` for anything else). */
export function registryObjectType(value: string): ObjectType | null {
	return value === 'page' || value === 'navigator' || value === 'collection' || value === 'record'
		? value
		: null;
}

export type FlowBlockRendererType = 'paragraph' | 'heading' | 'bulletItem' | 'codeBlock';

const BLOCK_RENDERERS: ReadonlySet<FlowBlockRendererType> = new Set([
	'paragraph',
	'heading',
	'bulletItem',
	'codeBlock'
]);

export function isRegisteredBlockType(value: string): value is FlowBlockRendererType {
	return BLOCK_RENDERERS.has(value as FlowBlockRendererType);
}
