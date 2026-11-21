import type { Component } from 'svelte';
import RecordPanelSurface from '../components/forms/RecordPanelSurface.svelte';

/** Ids in embed addresses: the characters of UUIDs and slugs, nothing else. */
const ID = /^[A-Za-z0-9_-]{1,64}$/;

export interface RecordSurfaceParams extends Record<string, unknown> {
	workspaceId: string;
	projectId: string;
	recordId: string;
}

/** One embeddable surface: how to read its parameters and where its embed address is. */
export interface EmbeddableSurface<P extends Record<string, unknown>> {
	kind: string;
	component: Component<P>;
	/** Parameters from an embed address (`/embed/record/{w}/{p}/{r}`); null when not valid. */
	params(url: URL): P | null;
	/** Parameters from the harness query (`w`, `p`, `r`); null when not valid. */
	fromQuery(query: URLSearchParams): P | null;
	/** The embed address that shows the surface. */
	href(params: P): string;
}

function ids(values: Array<string | null | undefined>): string[] | null {
	return values.every((value): value is string => !!value && ID.test(value))
		? (values as string[])
		: null;
}

export const recordSurface: EmbeddableSurface<RecordSurfaceParams> = {
	kind: 'record',
	component: RecordPanelSurface,
	params(url) {
		const parts = url.pathname.split('/').filter(Boolean);
		if (parts.length !== 5 || parts[0] !== 'embed' || parts[1] !== 'record') return null;
		const found = ids(parts.slice(2));
		return found ? { workspaceId: found[0], projectId: found[1], recordId: found[2] } : null;
	},
	fromQuery(query) {
		const found = ids([query.get('w'), query.get('p'), query.get('r')]);
		return found ? { workspaceId: found[0], projectId: found[1], recordId: found[2] } : null;
	},
	href: (params) =>
		`/embed/record/${encodeURIComponent(params.workspaceId)}/${encodeURIComponent(params.projectId)}/${encodeURIComponent(params.recordId)}`
};

/** Embeddable surfaces by kind. Settings, designers and admin screens are never registered. */
export const embeddableSurfaces = { record: recordSurface } as const;

export type EmbeddableKind = keyof typeof embeddableSurfaces;

/** The kind an embed address belongs to (`/embed/record/...` is `record`); null for any other. */
export function surfaceKindOf(url: URL): EmbeddableKind | null {
	const kind = url.pathname.split('/').filter(Boolean)[1];
	return kind && Object.prototype.hasOwnProperty.call(embeddableSurfaces, kind)
		? (kind as EmbeddableKind)
		: null;
}
