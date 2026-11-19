import {
	CalendarDays,
	Columns3,
	GanttChartSquare,
	Grid3x3,
	LayoutGrid,
	ListTree,
	Rows3,
	Table2
} from '@lucide/svelte';
import type { FormView } from '#lib/api/forms.js';

/** Icon per view type, from the lucide set the app already uses. */
export const VIEW_TYPE_ICONS = {
	grid: Table2,
	kanban: Columns3,
	calendar: CalendarDays,
	timeline: ListTree,
	card: LayoutGrid,
	pivot: Grid3x3,
	gantt: GanttChartSquare,
	detail: Rows3
} satisfies Record<FormView['view_type'], typeof Table2>;

/** Types offered under "New view…", in menu order. */
export const CREATABLE_VIEW_TYPES: FormView['view_type'][] = [
	'grid',
	'kanban',
	'calendar',
	'timeline',
	'card',
	'pivot',
	'gantt'
];

export function viewIsPrivate(view: Pick<FormView, 'config'> | null | undefined): boolean {
	return view?.config?.visibility === 'private';
}

export function viewIsDefault(view: Pick<FormView, 'config'> | null | undefined): boolean {
	return view?.config?.is_default === true;
}
