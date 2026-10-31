// Renderer registry (`contracts/ui-surface-v1.md` "Renderer registry"): avoids a route-page type
// switch that grows every version. v0.4 registers `page:canvas` and `navigator:tree` plus the
// four block-level renderers this alpha ships. An unregistered `${objectType}:${viewType}` fails
// to `read_only`/`unsupported` -- it must never let an arbitrary component path reach the CRDT.

// v0.6 registers `collection:table`, `collection:board` and `record:panel`. Their components are
// loaded through `load()` (a dynamic import), so their code is a separate chunk that only the
// Flow object route ever requests; `page:canvas` / `navigator:tree` are mounted statically by
// their routes and carry no loader.

import type { Component } from 'svelte';

export type ObjectType = 'page' | 'navigator' | 'collection' | 'record';
export type ViewType = 'canvas' | 'tree' | 'table' | 'board' | 'panel';
export type RendererKey = `${ObjectType}:${ViewType}`;
export type Capability = 'edit' | 'view';

export interface ViewRendererRegistration {
	readonly key: RendererKey;
	readonly capabilities: readonly Capability[];
	readonly fallback: 'unsupported' | 'read_only';
	// Svelte's `Component` is invariant in its props; each renderer declares its own.
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	readonly load?: () => Promise<Component<any>>;
}

const REGISTRY: Partial<Record<RendererKey, ViewRendererRegistration>> = {
	'page:canvas': { key: 'page:canvas', capabilities: ['edit', 'view'], fallback: 'read_only' },
	'navigator:tree': {
		key: 'navigator:tree',
		capabilities: ['edit', 'view'],
		fallback: 'read_only'
	},
	'collection:table': {
		key: 'collection:table',
		capabilities: ['edit', 'view'],
		fallback: 'unsupported',
		load: async () =>
			(await import('#lib/components/flow/collection/CollectionTable.svelte')).default
	},
	'collection:board': {
		key: 'collection:board',
		capabilities: ['edit', 'view'],
		fallback: 'unsupported',
		load: async () =>
			(await import('#lib/components/flow/collection/CollectionBoard.svelte')).default
	},
	'record:panel': {
		key: 'record:panel',
		capabilities: ['edit', 'view'],
		fallback: 'unsupported',
		load: async () => (await import('#lib/components/flow/collection/RecordPanel.svelte')).default
	}
};

export function resolveRenderer(
	objectType: ObjectType,
	viewType: ViewType
): ViewRendererRegistration | null {
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
