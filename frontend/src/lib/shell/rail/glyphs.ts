import type { Component } from 'svelte';
import Bot from '@lucide/svelte/icons/bot';
import FileText from '@lucide/svelte/icons/file-text';
import Folder from '@lucide/svelte/icons/folder';
import FolderKanban from '@lucide/svelte/icons/folder-kanban';
import House from '@lucide/svelte/icons/house';
import Landmark from '@lucide/svelte/icons/landmark';
import LayoutList from '@lucide/svelte/icons/layout-list';
import Search from '@lucide/svelte/icons/search';
import Settings from '@lucide/svelte/icons/settings';
import Table2 from '@lucide/svelte/icons/table-2';
import Workflow from '@lucide/svelte/icons/workflow';

export type Glyph = Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;

/** Lucide glyphs by the names `destinations.ts` and `shortcuts.svelte.ts` use. */
const GLYPHS: Record<string, Glyph> = {
	house: House,
	'folder-kanban': FolderKanban,
	workflow: Workflow,
	bot: Bot,
	landmark: Landmark,
	folder: Folder,
	'table-2': Table2,
	'file-text': FileText,
	'layout-list': LayoutList,
	settings: Settings,
	search: Search
};

/** The component for a glyph name; a plain folder for an unknown name. */
export function glyphFor(name: string): Glyph {
	return GLYPHS[name] ?? Folder;
}
