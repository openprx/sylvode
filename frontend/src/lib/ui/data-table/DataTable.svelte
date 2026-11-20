<script lang="ts" generics="T">
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUpDown from '@lucide/svelte/icons/arrow-up-down';
	import Inbox from '@lucide/svelte/icons/inbox';
	import SearchX from '@lucide/svelte/icons/search-x';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import WifiOff from '@lucide/svelte/icons/wifi-off';
	import Checkbox from '../checkbox/Checkbox.svelte';
	import { tooltip } from '../tooltip/tooltip';
	import { isTruncated } from '../context-line/context-line';
	import {
		ACTIONS_WIDTH,
		CHECK_WIDTH,
		ariaSort,
		columnWidth,
		frozenOffsets,
		headerState,
		isTwoLine,
		nextSort,
		setPage,
		setRange,
		setRow,
		tableWidth,
		twoLineColumns,
		type Column,
		type SortState,
		type TableState
	} from './data-table';
	import { rowFocus } from './row-focus';

	interface Props {
		rows: T[];
		columns: Column<T>[];
		rowId: (row: T) => string;
		/** Accessible name of the table. */
		label: string;
		/** Selection-box name for a row: "Select {title}"; default uses the row id. */
		rowLabel?: (row: T) => string;
		state?: TableState;
		stateAction?: () => void;
		stateActionLabel?: string;
		density?: 'comfortable' | 'compact';
		/** Row shown in the side panel: same marker as selected, plus aria-current. */
		currentId?: string | null;
		selectable?: boolean;
		selected?: Set<string>;
		sort?: SortState;
		onsort?: (sort: SortState) => void;
		/** Link target of the title (first column); Cmd/Ctrl-click opens it natively. */
		rowHref?: (row: T) => string;
		onrowopen?: (row: T, options: { replace: boolean }) => void;
		onclosepanel?: () => void;
		widths?: Record<string, number>;
		rowActions?: Snippet<[T]>;
		footer?: Snippet;
		rowAttributes?: (row: T) => Record<`data-${string}`, string>;
		/** Overrides the measured container width (previews, tests). */
		containerWidth?: number;
	}

	let {
		rows,
		columns,
		rowId,
		label,
		rowLabel,
		state: tableState = 'ready',
		stateAction,
		stateActionLabel,
		density = 'comfortable',
		currentId = null,
		selectable = false,
		selected = $bindable(new Set<string>()),
		sort = null,
		onsort,
		rowHref,
		onrowopen,
		onclosepanel,
		widths,
		rowActions,
		footer,
		rowAttributes,
		containerWidth
	}: Props = $props();

	let measured = $state(0);
	let anchorId = $state<string | null>(null);
	// Full text of cells that are cut by an ellipsis, shown in a tooltip.
	let cutText = $state<Record<string, string>>({});

	const width = $derived(containerWidth ?? measured);
	const twoLine = $derived(isTwoLine(width));
	const ids = $derived(rows.map(rowId));
	const header = $derived(headerState(ids, selected));
	const leading = $derived(selectable ? CHECK_WIDTH : 0);
	const offsets = $derived(frozenOffsets(columns, widths, leading));
	const total = $derived(tableWidth(columns, widths, selectable, !!rowActions));
	const stacked = $derived(twoLineColumns(columns));
	const lastFrozen = $derived(columns.filter((c) => c.frozen).at(-1)?.id);
	const showRows = $derived(tableState === 'ready');
	const blocked = $derived(
		tableState === 'empty' ||
			tableState === 'noResults' ||
			tableState === 'failed' ||
			tableState === 'offline'
	);
	const selectedCount = $derived(ids.filter((id) => selected.has(id)).length);
	const titleName = (row: T) => rowLabel?.(row) ?? rowId(row);

	function observe(node: HTMLElement) {
		if (containerWidth !== undefined) return;
		const observer = new ResizeObserver(() => (measured = node.clientWidth));
		observer.observe(node);
		measured = node.clientWidth;
		return { destroy: () => observer.disconnect() };
	}

	function changeRow(event: Event, id: string, shiftHeld = false) {
		const on = (event.target as HTMLInputElement).checked;
		selected = shiftHeld ? setRange(ids, selected, anchorId, id, on) : setRow(selected, id, on);
		anchorId = id;
	}
	let shiftOnClick = false;
	function rowKeydown(event: KeyboardEvent, id: string) {
		// Shift+Space extends the selection from the last toggled row.
		if (event.key === ' ' && event.shiftKey) {
			event.preventDefault();
			selected = setRange(ids, selected, anchorId, id, !selected.has(id));
			anchorId = id;
		}
	}
	function changeAll(event: Event) {
		selected = setPage(ids, selected, (event.target as HTMLInputElement).checked);
		anchorId = null;
	}

	function titleClick(event: MouseEvent, row: T) {
		if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
			return;
		if (!onrowopen) return;
		event.preventDefault();
		onrowopen(row, { replace: false });
	}
	function rowClick(event: MouseEvent, row: T) {
		if ((event.target as HTMLElement).closest('a, button, input, label, [data-no-row-open]'))
			return;
		onrowopen?.(row, { replace: false });
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape' || event.defaultPrevented) return;
		if (onclosepanel && currentId) {
			event.preventDefault();
			onclosepanel();
		} else if (selected.size > 0) {
			event.preventDefault();
			selected = new Set();
			anchorId = null;
		}
	}

	function noteTruncation(key: string, td: HTMLElement) {
		const cell = td.querySelector<HTMLElement>('.cell');
		cutText[key] = cell && isTruncated(cell) ? (cell.textContent ?? '').trim() : '';
	}

	const stateButtonLabel = $derived(
		tableState === 'noResults'
			? (stateActionLabel ?? $t('ui.dataTable.state.clearFilters'))
			: tableState === 'failed'
				? (stateActionLabel ?? $t('ui.dataTable.state.retry'))
				: tableState === 'offline'
					? (stateActionLabel ?? $t('ui.dataTable.state.retryConnection'))
					: stateActionLabel
	);
	const cellKey = (row: T, column: Column<T>) => `${rowId(row)}:${column.id}`;
</script>

{#snippet cellContent(row: T, column: Column<T>, first: boolean)}
	{#snippet inner()}
		<span
			class="cell"
			class:end={column.align === 'end'}
			use:tooltip={{ text: cutText[cellKey(row, column)] ?? '' }}
		>
			{@render column.cell(row)}
		</span>
	{/snippet}
	{#if first && (rowHref || onrowopen)}
		{#if rowHref}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- full path built by the caller -->
			<a
				class="title"
				href={rowHref(row)}
				data-record-title-link
				onclick={(e) => titleClick(e, row)}
			>
				{@render inner()}
			</a>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{:else}
			<button
				type="button"
				class="title"
				data-record-title-link
				onclick={(e) => titleClick(e, row)}
			>
				{@render inner()}
			</button>
		{/if}
	{:else}
		{@render inner()}
	{/if}
{/snippet}

{#snippet selectBox(row: T)}
	<!-- Checkbox keeps its own change handler, so the change event is read as it bubbles. -->
	<span
		class="box"
		role="presentation"
		onchange={(e) => {
			changeRow(e, rowId(row), shiftOnClick);
			shiftOnClick = false;
		}}
	>
		<Checkbox
			checked={selected.has(rowId(row))}
			aria-label={$t('ui.dataTable.selectRow', { values: { title: titleName(row) } })}
			onclick={(e) => (shiftOnClick = e.shiftKey)}
			onkeydown={(e) => rowKeydown(e, rowId(row))}
		/>
	</span>
{/snippet}

{#snippet stateBlock()}
	<div
		class="state"
		role={tableState === 'failed' ? 'alert' : 'status'}
		data-table-state={tableState}
	>
		<span class="state-glyph">
			{#if tableState === 'empty'}<Inbox size={24} aria-hidden="true" />
			{:else if tableState === 'noResults'}<SearchX size={24} aria-hidden="true" />
			{:else if tableState === 'offline'}<WifiOff size={24} aria-hidden="true" />
			{:else}<CircleAlert size={24} aria-hidden="true" />{/if}
		</span>
		<p class="state-title">{$t(`ui.dataTable.state.${tableState}.title`)}</p>
		<p class="state-body">{$t(`ui.dataTable.state.${tableState}.body`)}</p>
		{#if stateButtonLabel && stateAction}
			<button type="button" class="state-action" onclick={stateAction}>{stateButtonLabel}</button>
		{/if}
	</div>
{/snippet}

<div
	class="ui-data-table"
	data-density={density}
	data-ui="data-table"
	data-two-line={twoLine || undefined}
	use:observe
	onkeydown={onKeydown}
	role="presentation"
>
	<div class="sr-only" role="status">
		{#if selectedCount === 1}{$t('ui.dataTable.selectedOne')}{:else if selectedCount > 1}{$t(
				'ui.dataTable.selectedMany',
				{ values: { count: selectedCount } }
			)}{/if}
	</div>

	{#if twoLine}
		<ul
			class="list"
			aria-label={label}
			aria-busy={tableState === 'loading' || undefined}
			use:rowFocus={{ rows: '[data-row]' }}
		>
			{#if tableState === 'loading'}
				{#each [0, 1, 2, 3, 4] as n (n)}
					<li class="row two" aria-hidden="true">
						<span class="skeleton wide"></span>
					</li>
				{/each}
			{:else if showRows}
				{#each rows as row (rowId(row))}
					{@const id = rowId(row)}
					<li
						class="row two"
						data-row
						data-selected={selected.has(id) || currentId === id ? '' : undefined}
						aria-current={currentId === id ? 'true' : undefined}
						onclick={(e) => rowClick(e, row)}
						{...rowAttributes?.(row)}
					>
						<span class="check-cell"
							>{#if selectable}{@render selectBox(row)}{/if}</span
						>
						<span class="stack">
							<span class="line one">
								{#each stacked.primary as column, i (column.id)}
									{@render cellContent(row, column, i === 0 && column === columns[0])}
								{/each}
							</span>
							<span class="line two-meta">
								{#each stacked.secondary as column (column.id)}
									<span class="meta">{@render column.cell(row)}</span>
								{/each}
							</span>
						</span>
						{#if rowActions}<span class="row-actions">{@render rowActions(row)}</span>{/if}
					</li>
				{/each}
			{/if}
		</ul>
		{#if blocked}{@render stateBlock()}{/if}
	{:else}
		<div class="scroll">
			<table
				aria-label={label}
				aria-busy={tableState === 'loading' || undefined}
				style:width="{total}px"
			>
				<colgroup>
					{#if selectable}<col style:width="{CHECK_WIDTH}px" />{/if}
					{#each columns as column (column.id)}<col
							style:width="{columnWidth(column, widths)}px"
						/>{/each}
					{#if rowActions}<col style:width="{ACTIONS_WIDTH}px" />{/if}
				</colgroup>
				<thead>
					<tr>
						{#if selectable}
							<th scope="col" class="check-cell frozen" style:left="0px">
								<span class="box" role="presentation" onchange={changeAll}>
									<Checkbox
										checked={header === 'all'}
										indeterminate={header === 'some'}
										disabled={ids.length === 0}
										aria-label={$t('ui.dataTable.selectAll')}
									/>
								</span>
							</th>
						{/if}
						{#each columns as column (column.id)}
							{@const dir = ariaSort(sort, column.id, column.sortable)}
							<th
								scope="col"
								class:end={column.align === 'end'}
								class:frozen={column.frozen}
								style:left={column.frozen ? `${offsets[column.id]}px` : undefined}
								aria-sort={dir}
							>
								{#if column.sortable}
									<button
										type="button"
										class="sort"
										class:sorted={dir === 'ascending' || dir === 'descending'}
										onclick={() => onsort?.(nextSort(sort, column.id))}
									>
										<span class="sort-label">{column.header}</span>
										{#if dir === 'ascending'}<ArrowUp size={14} aria-hidden="true" />
										{:else if dir === 'descending'}<ArrowDown size={14} aria-hidden="true" />
										{:else}<ArrowUpDown size={14} aria-hidden="true" class="idle" />{/if}
									</button>
								{:else}
									<span class="head-label">{column.header}</span>
								{/if}
							</th>
						{/each}
						{#if rowActions}<th scope="col"
								><span class="sr-only">{$t('ui.dataTable.actions')}</span></th
							>{/if}
					</tr>
				</thead>
				<tbody use:rowFocus={{ rows: 'tr[data-row]' }}>
					{#if tableState === 'loading'}
						{#each [0, 1, 2, 3, 4] as n (n)}
							<tr class="skeleton-row" aria-hidden="true">
								{#if selectable}<td class="check-cell"><span class="skeleton box"></span></td>{/if}
								{#each columns as column (column.id)}
									<td><span class="skeleton"></span></td>
								{/each}
								{#if rowActions}<td></td>{/if}
							</tr>
						{/each}
					{:else if showRows}
						{#each rows as row (rowId(row))}
							{@const id = rowId(row)}
							<tr
								data-row
								data-selected={selected.has(id) || currentId === id ? '' : undefined}
								aria-current={currentId === id ? 'true' : undefined}
								onclick={(e) => rowClick(e, row)}
								{...rowAttributes?.(row)}
							>
								{#if selectable}
									<td class="check-cell frozen" style:left="0px">{@render selectBox(row)}</td>
								{/if}
								{#each columns as column, ci (column.id)}
									<td
										class:end={column.align === 'end'}
										class:frozen={column.frozen}
										class:edge={column.id === lastFrozen}
										style:left={column.frozen ? `${offsets[column.id]}px` : undefined}
										onpointerenter={(e) => noteTruncation(cellKey(row, column), e.currentTarget)}
									>
										{@render cellContent(row, column, ci === 0)}
									</td>
								{/each}
								{#if rowActions}<td class="row-actions">{@render rowActions(row)}</td>{/if}
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
			{#if blocked}{@render stateBlock()}{/if}
		</div>
	{/if}
	{#if footer}<div class="footer">{@render footer()}</div>{/if}
</div>

<style>
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		margin: -1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	.ui-data-table {
		position: relative;
		min-width: 0;
		background: var(--rf-bg);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-density-text);
		line-height: var(--rf-leading-body-tight);
	}
	.scroll {
		overflow-x: auto;
	}
	table {
		border-collapse: separate;
		border-spacing: 0;
		table-layout: fixed;
		min-width: 100%;
	}
	th,
	td {
		position: relative;
		box-sizing: border-box;
		padding: 0 var(--rf-density-cell-pad);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		background: var(--rf-bg);
		text-align: start;
		vertical-align: middle;
	}
	th {
		position: sticky;
		top: 0;
		z-index: var(--rf-z-sticky);
		height: var(--rf-density-control);
		border-bottom-color: var(--rf-line-strong);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		font-weight: var(--rf-weight-regular);
	}
	td {
		height: var(--rf-density-row);
	}
	th.frozen,
	td.frozen {
		position: sticky;
		z-index: calc(var(--rf-z-sticky) + 1);
	}
	td.edge {
		border-inline-end: var(--rf-border-width) solid var(--rf-line-strong);
	}
	.end {
		text-align: end;
		font-variant-numeric: tabular-nums;
	}
	.box {
		display: contents;
	}
	.check-cell {
		padding: 0 0 0 var(--rf-space-5);
	}
	.cell {
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.title {
		display: block;
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		font-weight: var(--rf-weight-medium);
		text-align: start;
		text-decoration: none;
		cursor: pointer;
	}
	.title:hover {
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.title:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
		border-radius: 2px;
	}
	.head-label,
	.sort-label {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sort {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		max-width: 100%;
		height: 24px;
		margin-inline-start: calc(var(--rf-space-4) * -1);
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		font: inherit;
		cursor: pointer;
	}
	.sort :global(.idle) {
		color: var(--rf-control);
	}
	.sort.sorted {
		color: var(--rf-text);
		font-weight: var(--rf-weight-semibold);
	}
	.sort:hover {
		background: var(--rf-hover);
	}
	.sort:active {
		background: var(--rf-pressed);
	}
	.sort:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	tr[data-row] {
		cursor: default;
	}
	tr[data-row]:hover > td {
		background: var(--rf-hover);
	}
	tr[data-row]:active > td {
		background: var(--rf-pressed);
	}
	tr[data-selected] > td,
	tr[data-selected]:hover > td {
		background: var(--rf-selected);
	}
	/* Selection marker: a 2px primary bar at the leading edge, half the row height (at least 16). */
	tr[data-selected] > td:first-child::before,
	.row[data-selected]::before {
		content: '';
		position: absolute;
		inset-inline-start: 0;
		top: 50%;
		width: 2px;
		height: max(16px, 50%);
		transform: translateY(-50%);
		background: var(--rf-primary);
	}
	/* Keyboard focus: a 2px ring around the whole row, drawn inside the cells so frozen cells keep it. */
	tr[data-row]:has(:focus-visible) > td {
		box-shadow:
			inset 0 2px 0 var(--rf-focus),
			inset 0 -2px 0 var(--rf-focus);
	}
	tr[data-row]:has(:focus-visible) > td:first-child {
		box-shadow:
			inset 2px 0 0 var(--rf-focus),
			inset 0 2px 0 var(--rf-focus),
			inset 0 -2px 0 var(--rf-focus);
	}
	tr[data-row]:has(:focus-visible) > td:last-child {
		box-shadow:
			inset -2px 0 0 var(--rf-focus),
			inset 0 2px 0 var(--rf-focus),
			inset 0 -2px 0 var(--rf-focus);
	}
	.row-actions {
		text-align: end;
		color: var(--rf-secondary);
	}
	.skeleton {
		display: block;
		width: 70%;
		height: 12px;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	.skeleton.box {
		width: 18px;
		height: 18px;
	}
	.skeleton.wide {
		width: 60%;
	}
	/* Two-line layout for narrow containers. */
	.list {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row.two {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		box-sizing: border-box;
		min-height: var(--rf-density-row-two-line);
		padding: var(--rf-space-3) var(--rf-density-cell-pad);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
	}
	.row.two[data-row]:hover {
		background: var(--rf-hover);
	}
	.row.two[data-selected],
	.row.two[data-selected]:hover {
		background: var(--rf-selected);
	}
	.row.two[data-row]:has(:focus-visible) {
		box-shadow: inset 0 0 0 var(--rf-border-width-focus) var(--rf-focus);
	}
	.check-cell:empty {
		display: none;
	}
	.stack {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}
	.line {
		display: flex;
		gap: var(--rf-space-4);
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
	}
	.line.two-meta {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.meta {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.state {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-3);
		padding: var(--rf-space-9) var(--rf-space-6);
		text-align: center;
	}
	.state-glyph {
		color: var(--rf-secondary);
	}
	.state-title {
		margin: 0;
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-semibold);
	}
	.state-body {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.state-action {
		height: var(--rf-control-height);
		margin-top: var(--rf-space-3);
		padding: 0 var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-text);
		border-radius: var(--rf-radius-button);
		background: var(--rf-surface-white);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-button);
		font-weight: var(--rf-weight-medium);
		cursor: pointer;
	}
	.state-action:hover {
		background: var(--rf-hover);
	}
	.state-action:active {
		background: var(--rf-pressed);
	}
	.state-action:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.footer {
		padding: var(--rf-space-4) var(--rf-density-cell-pad);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	@media (pointer: coarse) {
		.sort,
		.state-action {
			min-height: var(--rf-touch-target);
		}
	}
</style>
