<script lang="ts" module>
	export interface GridBadge {
		/** Data attribute name without the `data-` prefix, kept for tests and tooling. */
		attr: string;
		value: string;
		label: string;
		tone: 'amber' | 'slate' | 'blue' | 'cyan';
	}
</script>

<script lang="ts">
	/**
	 * The record table. A row opens the record side panel: a click anywhere on the row except its
	 * own controls, or Enter on the title link. The title is a real link to the full record page,
	 * so Cmd/Ctrl-click, middle-click and "Open in new tab" work natively. Rows use a roving
	 * tabindex: only one title link is in the Tab order and the arrow keys move between rows.
	 */
	import { tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		ArrowDown,
		ArrowUp,
		ArrowUpDown,
		ChevronLeft,
		ChevronRight,
		Pencil,
		Trash2
	} from '@lucide/svelte';
	import type { FormField, FormRecord } from '#lib/api/forms.js';
	import type { FieldDisplayContext } from '#lib/forms/field-display.js';
	import Button from '#lib/components/Button.svelte';
	import FieldValue from './FieldValue.svelte';

	interface Props {
		records: FormRecord[];
		fields: FormField[];
		ctx: FieldDisplayContext;
		/** Record shown in the side panel, if any. */
		currentId: string | null;
		sort: { field: string; direction: 'asc' | 'desc' } | null;
		sortableKeys: string[];
		page: number;
		pageCount: number;
		total: number;
		deletingId: string | null;
		recordHref: (recordId: string) => string;
		headerBadges: (field: FormField) => GridBadge[];
		cellBadges: (field: FormField, record: FormRecord) => GridBadge[];
		canEdit: (record: FormRecord) => boolean;
		canDelete: (record: FormRecord) => boolean;
		onopen: (record: FormRecord, options: { replace: boolean }) => void;
		onclosepanel: () => void;
		onfocuspanel: () => void;
		onedit: (record: FormRecord) => void;
		ondelete: (record: FormRecord) => void;
		onsort: (sort: { field: string; direction: 'asc' | 'desc' } | null) => void;
		ongotopage: (page: number) => void;
		onannounce: (message: string) => void;
	}

	let {
		records,
		fields,
		ctx,
		currentId,
		sort,
		sortableKeys,
		page,
		pageCount,
		total,
		deletingId,
		recordHref,
		headerBadges,
		cellBadges,
		canEdit,
		canDelete,
		onopen,
		onclosepanel,
		onfocuspanel,
		onedit,
		ondelete,
		onsort,
		ongotopage,
		onannounce
	}: Props = $props();

	let tbody = $state<HTMLElement | null>(null);
	let focusedId = $state<string | null>(null);

	const rovingId = $derived(
		[currentId, focusedId].find((id) => id && records.some((record) => record.id === id)) ??
			records[0]?.id ??
			null
	);

	const IGNORE =
		'a, button, input, select, textarea, label, summary, [role="checkbox"], [data-row-ignore]';

	const fieldLabel = (field: FormField) => field.label?.trim() || field.key.replaceAll('_', ' ');

	const toneClass: Record<GridBadge['tone'], string> = {
		amber: 'bg-warning-soft text-warning',
		slate: 'bg-surface text-fg-muted',
		blue: 'bg-surface text-fg',
		cyan: 'bg-surface text-fg'
	};

	export function focusRow(recordId: string | null) {
		const id = recordId && records.some((record) => record.id === recordId) ? recordId : rovingId;
		if (!id) return;
		focusedId = id;
		const link = tbody?.querySelector<HTMLElement>(`[data-record-title-link="${CSS.escape(id)}"]`);
		link?.focus({ preventScroll: true });
		link
			?.closest('tr')
			?.querySelector('td')
			?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
	}

	/** Focus the row at `index` of the current page, or the one now at that index. */
	export function focusIndex(index: number) {
		const record = records[Math.min(Math.max(index, 0), records.length - 1)];
		if (record) focusRow(record.id);
	}

	export function rowIndex(recordId: string): number {
		return records.findIndex((record) => record.id === recordId);
	}

	function modified(event: MouseEvent | KeyboardEvent): boolean {
		return event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;
	}

	function onRowClick(event: MouseEvent, record: FormRecord) {
		const target = event.target as HTMLElement | null;
		const control = target?.closest(IGNORE);
		if (control && !control.hasAttribute('data-record-title-link')) return;
		if (control && (modified(event) || event.button !== 0)) return;
		if (window.getSelection()?.toString()) return;
		event.preventDefault();
		focusedId = record.id;
		onopen(record, { replace: false });
	}

	async function move(record: FormRecord, delta: number | 'first' | 'last') {
		const index = rowIndex(record.id);
		let targetIndex = delta === 'first' ? 0 : delta === 'last' ? records.length - 1 : index + delta;
		if (targetIndex >= records.length || targetIndex < 0) {
			const nextPage = page + (targetIndex < 0 ? -1 : 1);
			if (nextPage < 1 || nextPage > pageCount) return;
			ongotopage(nextPage);
			onannounce(
				$t('forms.recordPanel.pageAnnounce', { values: { page: nextPage, count: pageCount } })
			);
			await tick();
			targetIndex = targetIndex < 0 ? records.length - 1 : 0;
		}
		const target = records[targetIndex];
		if (!target) return;
		focusRow(target.id);
		if (currentId) onopen(target, { replace: true });
	}

	function onKeydown(event: KeyboardEvent, record: FormRecord) {
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			if (event.altKey) return;
			event.preventDefault();
			void move(record, event.key === 'ArrowDown' ? 1 : -1);
		} else if (event.key === 'Home' || event.key === 'End') {
			event.preventDefault();
			void move(record, event.key === 'Home' ? 'first' : 'last');
		} else if (event.key === 'Enter') {
			event.preventDefault();
			if (event.metaKey || event.ctrlKey) {
				window.open(recordHref(record.id), '_blank', 'noopener');
				return;
			}
			if (currentId === record.id) onfocuspanel();
			else onopen(record, { replace: false });
		} else if (event.key === 'Escape' && currentId) {
			event.preventDefault();
			onclosepanel();
		} else if (event.key === 'F6' && currentId) {
			event.preventDefault();
			onfocuspanel();
		}
	}

	function cycleSort(key: string) {
		if (sort?.field !== key) onsort({ field: key, direction: 'asc' });
		else if (sort.direction === 'asc') onsort({ field: key, direction: 'desc' });
		else onsort(null);
	}

	function ariaSort(key: string): 'ascending' | 'descending' | 'none' {
		if (sort?.field !== key) return 'none';
		return sort.direction === 'desc' ? 'descending' : 'ascending';
	}

	const headerButton =
		'inline-flex min-h-6 items-center gap-1 rounded-control text-start font-medium hover:text-fg';
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- title links are full record page paths built by the caller -->

{#snippet sortIcon(key: string)}
	{#if sort?.field === key}
		{#if sort.direction === 'desc'}<ArrowDown
				class="h-3.5 w-3.5"
				aria-hidden="true"
			/>{:else}<ArrowUp class="h-3.5 w-3.5" aria-hidden="true" />{/if}
	{:else}
		<ArrowUpDown class="h-3.5 w-3.5 text-fg-muted" aria-hidden="true" />
	{/if}
{/snippet}

<div class="overflow-x-auto" data-view-renderer="grid">
	<p id="record-table-hint" class="sr-only">{$t('forms.recordPanel.tableHint')}</p>
	<table
		class="min-w-full divide-y divide-line text-sm"
		aria-describedby="record-table-hint"
	>
		<thead class="bg-nav">
			<tr>
				<th
					class="w-56 px-4 py-3 text-start font-medium text-fg-muted"
					aria-sort={ariaSort('title')}
				>
					<button
						type="button"
						class={headerButton}
						data-grid-sort="title"
						onclick={() => cycleSort('title')}
						>{$t('forms.recordTitle')}{@render sortIcon('title')}</button
					>
				</th>
				{#each fields as field (field.key)}
					{@const sortable = sortableKeys.includes(field.key)}
					<th
						class="px-4 py-3 text-start font-medium text-fg-muted"
						aria-sort={sortable ? ariaSort(field.key) : undefined}
					>
						<div class="flex flex-wrap items-center gap-1.5">
							{#if sortable}
								<button
									type="button"
									class={headerButton}
									data-grid-sort={field.key}
									onclick={() => cycleSort(field.key)}
									>{fieldLabel(field)}{@render sortIcon(field.key)}</button
								>
							{:else}
								<span>{fieldLabel(field)}</span>
							{/if}
							{#each headerBadges(field) as badge (badge.attr)}
								<span
									class="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[10px] font-medium uppercase {toneClass[
										badge.tone
									]}"
									{...{ [`data-${badge.attr}`]: badge.value }}>{badge.label}</span
								>
							{/each}
						</div>
					</th>
				{/each}
				<th class="px-4 py-3 text-end font-medium text-fg-muted"
					>{$t('common.actions')}</th
				>
			</tr>
		</thead>
		<tbody bind:this={tbody} class="divide-y divide-line">
			{#each records as record (record.id)}
				{@const selected = record.id === currentId}
				<tr
					class="cursor-pointer transition-colors duration-[120ms] ease-out motion-reduce:transition-none {selected
						? 'bg-selected'
						: 'hover:bg-surface'}"
					aria-current={selected ? 'true' : undefined}
					data-record-row={record.id}
					onclick={(event) => onRowClick(event, record)}
				>
					<td class="relative px-4 py-3">
						{#if selected}
							<span
								class="absolute inset-y-0 start-0 w-[3px] bg-primary"
								aria-hidden="true"
							></span>
						{/if}
						<a
							href={recordHref(record.id)}
							class="rounded-[var(--radius-sm)] font-medium text-fg hover:underline"
							tabindex={record.id === rovingId ? 0 : -1}
							aria-controls="record-panel"
							aria-expanded={selected}
							data-record-title-link={record.id}
							onfocus={() => (focusedId = record.id)}
							onkeydown={(event) => onKeydown(event, record)}>{record.title}</a
						>
						<div class="mt-1 font-mono text-xs text-fg-muted" dir="ltr">
							{record.id.slice(0, 8)}
						</div>
					</td>
					{#each fields as field (field.key)}
						<td class="max-w-64 px-4 py-3 text-fg">
							<div class="flex min-w-0 flex-wrap items-center gap-1.5">
								<span class="min-w-0 max-w-full truncate"
									><FieldValue
										{field}
										value={record.values[field.key]}
										variant="cell"
										{ctx}
									/></span
								>
								{#each cellBadges(field, record) as badge (badge.attr)}
									<span
										class="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[10px] font-medium uppercase {toneClass[
											badge.tone
										]}"
										{...{ [`data-${badge.attr}`]: badge.value }}>{badge.label}</span
									>
								{/each}
							</div>
						</td>
					{/each}
					<td class="px-4 py-3 text-end whitespace-nowrap">
						{#if canEdit(record)}
							<button
								type="button"
								class="me-1 inline-flex h-8 w-8 items-center justify-center rounded-control text-fg hover:bg-surface"
								tabindex={record.id === rovingId ? 0 : -1}
								onclick={() => onedit(record)}
								aria-label={$t('forms.editRecord')}
								title={$t('forms.editRecord')}><Pencil class="h-4 w-4" aria-hidden="true" /></button
							>
						{/if}
						{#if canDelete(record)}
							<button
								type="button"
								class="inline-flex h-8 w-8 items-center justify-center rounded-control text-danger hover:bg-danger-soft disabled:text-control"
								tabindex={record.id === rovingId ? 0 : -1}
								disabled={deletingId === record.id}
								onclick={() => ondelete(record)}
								aria-label={$t('forms.deleteRecord')}
								title={$t('forms.deleteRecord')}
								><Trash2 class="h-4 w-4" aria-hidden="true" /></button
							>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

{#if pageCount > 1}
	<div
		class="flex flex-col gap-2 border-t border-line px-4 py-3 text-sm sm:flex-row sm:items-center sm:justify-between"
		data-record-pagination
	>
		<span class="text-fg-muted">
			{$t('forms.recordPageLabel', { values: { page, total: pageCount, count: total } })}
		</span>
		<div class="flex items-center gap-2">
			<Button
				size="sm"
				variant="secondary"
				disabled={page <= 1}
				onclick={() => ongotopage(page - 1)}
			>
				<ChevronLeft class="me-2 h-4 w-4 rtl:-scale-x-100" aria-hidden="true" />
				{$t('forms.previousPage')}
			</Button>
			<Button
				size="sm"
				variant="secondary"
				disabled={page >= pageCount}
				onclick={() => ongotopage(page + 1)}
			>
				{$t('forms.nextPage')}
				<ChevronRight class="ms-2 h-4 w-4 rtl:-scale-x-100" aria-hidden="true" />
			</Button>
		</div>
	</div>
{/if}
