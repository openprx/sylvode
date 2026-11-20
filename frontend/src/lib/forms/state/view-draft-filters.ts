import type {
	ViewFilterDraft,
	ViewFilterGroupDraft,
	ViewFilterLogic
} from '#lib/forms/page-types.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { syncLegacyViewFilterFields } from '#lib/forms/view-draft-parse.js';

export class ViewDraftFiltersController extends FormsController {
	setViewDraftFilterGroups = (filterGroups: ViewFilterGroupDraft[]) => {
		const nextGroups = filterGroups
			.map((group) => ({
				logic: group.logic,
				filters: group.filters.slice(0, 5),
				disabled: group.disabled || undefined
			}))
			.filter((group) => group.filters.length > 0)
			.slice(0, 5);

		this.s.viewDraft = {
			...this.s.viewDraft,
			filterGroups: nextGroups,
			...syncLegacyViewFilterFields(nextGroups)
		};
	};

	updateViewDraftFilterRootLogic = (logic: ViewFilterLogic) => {
		this.s.viewDraft = { ...this.s.viewDraft, filterRootLogic: logic };
	};

	setViewDraftFilters = (filters: ViewFilterDraft[], groupIndex = 0) => {
		const nextFilters = filters.slice(0, 5);

		const nextGroups =
			this.s.viewDraft.filterGroups.length > 0
				? this.s.viewDraft.filterGroups.map((group) => ({ ...group, filters: [...group.filters] }))
				: [{ logic: 'all' as ViewFilterLogic, filters: [] }];

		nextGroups[groupIndex] = {
			logic: nextGroups[groupIndex]?.logic ?? 'all',
			filters: nextFilters
		};

		this.setViewDraftFilterGroups(nextGroups);
	};

	updateViewDraftFilterGroupLogic = (groupIndex: number, logic: ViewFilterLogic) => {
		const nextGroups = this.s.viewDraft.filterGroups.map((group, itemIndex) =>
			itemIndex === groupIndex ? { ...group, logic } : group
		);

		this.setViewDraftFilterGroups(nextGroups);
	};

	toggleViewDraftFilterGroupDisabled = (groupIndex: number) => {
		this.syncVisibleFilterGroups();

		const nextGroups = this.visibleViewDraftFilterGroups().map((group, itemIndex) => ({
			...group,
			filters: group.filters.map((filter) => ({ ...filter })),
			disabled: itemIndex === groupIndex ? !group.disabled : group.disabled || undefined
		}));

		this.setViewDraftFilterGroups(nextGroups);
	};

	addViewDraftFilterGroup = () => {
		if (this.s.viewDraft.filterGroups.length >= 5) return;

		this.setViewDraftFilterGroups([
			...this.s.viewDraft.filterGroups,
			{
				logic: 'any',
				filters: [{ field: '', operator: 'contains', value: '' }]
			}
		]);
	};

	removeViewDraftFilterGroup = (groupIndex: number) => {
		this.setViewDraftFilterGroups(
			this.s.viewDraft.filterGroups.filter((_, itemIndex) => itemIndex !== groupIndex)
		);
	};

	canDuplicateViewDraftFilterGroup = (groupIndex: number): boolean => {
		const group = this.visibleViewDraftFilterGroups()[groupIndex];

		if (!group || group.filters.length === 0) return false;

		return (
			this.s.viewDraft.filterGroups.length < 5 && !this.wouldExceedFilterLimit(group.filters.length)
		);
	};

	duplicateViewDraftFilterGroup = (groupIndex: number) => {
		if (!this.canDuplicateViewDraftFilterGroup(groupIndex)) return;

		this.syncVisibleFilterGroups();

		const nextGroups = this.s.viewDraft.filterGroups.map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));

		const sourceGroup = nextGroups[groupIndex];

		if (!sourceGroup) return;

		nextGroups.splice(groupIndex + 1, 0, {
			logic: sourceGroup.logic,
			filters: sourceGroup.filters.map((filter) => ({ ...filter }))
		});

		this.setViewDraftFilterGroups(nextGroups);
	};

	canMergeViewDraftFilterGroup = (groupIndex: number): boolean => {
		const groups = this.visibleViewDraftFilterGroups();
		const sourceGroup = groups[groupIndex];
		const previousGroup = groups[groupIndex - 1];

		if (groupIndex <= 0 || !sourceGroup || !previousGroup) return false;

		return previousGroup.filters.length + sourceGroup.filters.length <= 5;
	};

	mergeViewDraftFilterGroupWithPrevious = (groupIndex: number) => {
		if (!this.canMergeViewDraftFilterGroup(groupIndex)) return;

		const nextGroups = this.visibleViewDraftFilterGroups().map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));

		const sourceGroup = nextGroups[groupIndex];
		const previousGroup = nextGroups[groupIndex - 1];

		if (!sourceGroup || !previousGroup) return;

		previousGroup.filters = [
			...previousGroup.filters,
			...sourceGroup.filters.map((filter) => ({ ...filter }))
		];

		nextGroups.splice(groupIndex, 1);
		this.setViewDraftFilterGroups(nextGroups);
	};

	moveViewDraftFilterGroup = (groupIndex: number, direction: -1 | 1) => {
		const targetIndex = groupIndex + direction;

		if (targetIndex < 0 || targetIndex >= this.s.viewDraft.filterGroups.length) return;

		const nextGroups = this.s.viewDraft.filterGroups.map((group) => ({
			...group,
			filters: [...group.filters]
		}));

		[nextGroups[groupIndex], nextGroups[targetIndex]] = [
			nextGroups[targetIndex],
			nextGroups[groupIndex]
		];
		this.setViewDraftFilterGroups(nextGroups);
	};

	visibleViewDraftFilterGroups = (): ViewFilterGroupDraft[] => {
		if (this.s.viewDraft.filterGroups.length > 0) return this.s.viewDraft.filterGroups;

		return [{ logic: 'all', filters: this.s.viewDraft.filters }];
	};

	hasReachedFilterLimit = (): boolean => {
		return (
			this.s.viewDraft.filterGroups.reduce((count, group) => count + group.filters.length, 0) >= 10
		);
	};

	wouldExceedFilterLimit = (extraCount: number): boolean => {
		return (
			this.s.viewDraft.filterGroups.reduce((count, group) => count + group.filters.length, 0) +
				extraCount >
			10
		);
	};

	syncVisibleFilterGroups = () => {
		if (this.s.viewDraft.filterGroups.length === 0) {
			this.setViewDraftFilterGroups([{ logic: 'all', filters: this.s.viewDraft.filters }]);
		}
	};

	updateViewDraftFilter = (index: number, patch: Partial<ViewFilterDraft>, groupIndex = 0) => {
		this.syncVisibleFilterGroups();

		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};
		const nextFilters = sourceGroup.filters.map((filter, itemIndex) =>
			itemIndex === index ? { ...filter, ...patch } : filter
		);

		this.setViewDraftFilters(nextFilters, groupIndex);
	};

	toggleViewDraftFilterDisabled = (groupIndex: number, filterIndex: number) => {
		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};
		const sourceFilter = sourceGroup.filters[filterIndex];

		if (!sourceFilter) return;

		this.updateViewDraftFilter(filterIndex, { disabled: !sourceFilter.disabled }, groupIndex);
	};

	addViewDraftFilter = (groupIndex = 0) => {
		if (this.hasReachedFilterLimit()) return;

		this.syncVisibleFilterGroups();

		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};

		this.setViewDraftFilters(
			[...sourceGroup.filters, { field: '', operator: 'contains', value: '' }],
			groupIndex
		);
	};

	removeViewDraftFilter = (index: number, groupIndex = 0) => {
		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};

		this.setViewDraftFilters(
			sourceGroup.filters.filter((_, itemIndex) => itemIndex !== index),
			groupIndex
		);
	};

	duplicateViewDraftFilter = (groupIndex: number, filterIndex: number) => {
		if (this.hasReachedFilterLimit()) return;

		this.syncVisibleFilterGroups();

		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};
		const sourceFilter = sourceGroup.filters[filterIndex];

		if (!sourceFilter) return;

		const nextFilters = sourceGroup.filters.map((filter) => ({ ...filter }));

		nextFilters.splice(filterIndex + 1, 0, { ...sourceFilter });
		this.setViewDraftFilters(nextFilters, groupIndex);
	};

	canSplitViewDraftFilterToGroup = (groupIndex: number, filterIndex: number): boolean => {
		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex];

		return Boolean(
			sourceGroup &&
			sourceGroup.filters[filterIndex] &&
			sourceGroup.filters.length > 1 &&
			this.visibleViewDraftFilterGroups().length < 5
		);
	};

	splitViewDraftFilterToGroup = (groupIndex: number, filterIndex: number) => {
		if (!this.canSplitViewDraftFilterToGroup(groupIndex, filterIndex)) return;

		const nextGroups = this.visibleViewDraftFilterGroups().map((group) => ({
			logic: group.logic,
			filters: group.filters.map((filter) => ({ ...filter }))
		}));

		const sourceGroup = nextGroups[groupIndex];

		if (!sourceGroup) return;

		const [sourceFilter] = sourceGroup.filters.splice(filterIndex, 1);

		if (!sourceFilter) return;

		nextGroups.splice(groupIndex + 1, 0, {
			logic: sourceGroup.logic,
			filters: [{ ...sourceFilter }]
		});
		this.setViewDraftFilterGroups(nextGroups);
	};

	moveViewDraftFilter = (groupIndex: number, filterIndex: number, direction: -1 | 1) => {
		const sourceGroup = this.visibleViewDraftFilterGroups()[groupIndex] ?? {
			logic: 'all',
			filters: []
		};
		const targetIndex = filterIndex + direction;

		if (targetIndex < 0 || targetIndex >= sourceGroup.filters.length) return;

		const nextFilters = sourceGroup.filters.map((filter) => ({ ...filter }));

		[nextFilters[filterIndex], nextFilters[targetIndex]] = [
			nextFilters[targetIndex],
			nextFilters[filterIndex]
		];
		this.setViewDraftFilters(nextFilters, groupIndex);
	};

	validViewFilterGroups = (): ViewFilterGroupDraft[] => {
		return this.s.viewDraft.filterGroups
			.map((group) => ({
				logic: group.logic,
				filters: group.filters
					.filter(
						(filter) => filter.field && (filter.operator === 'not_empty' || filter.value.trim())
					)
					.map((filter) => ({
						field: filter.field,
						operator: filter.operator,
						value: filter.operator === 'not_empty' ? '' : filter.value.trim(),
						disabled: filter.disabled || undefined
					})),
				disabled: group.disabled || undefined
			}))
			.filter((group) => group.filters.length > 0);
	};

	viewFilterExpressionConfigFromGroups = (
		filterGroups: ViewFilterGroupDraft[]
	): Record<string, unknown> | null => {
		if (filterGroups.length === 0) return null;

		return {
			logic: this.s.viewDraft.filterRootLogic,
			children: filterGroups.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				children: group.filters.map((filter) => ({
					disabled: filter.disabled || undefined,
					filter: {
						field: filter.field,
						operator: filter.operator,
						value: filter.operator === 'not_empty' ? '' : filter.value.trim()
					}
				}))
			}))
		};
	};
}
