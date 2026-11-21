<script lang="ts">
	import { t } from 'svelte-i18n';
	import DataTable from './DataTable.svelte';
	import type { Column, SortState, TableState } from './data-table';

	interface Row {
		id: string;
		name: string;
		owner: string;
		amount: number;
		status: string;
	}
	interface Props {
		state?: TableState;
		density?: 'comfortable' | 'compact';
		/** CSS width of the frame; the table follows it. */
		frame?: number;
		selectable?: boolean;
	}
	let {
		state: tableState = 'ready',
		density = 'comfortable',
		frame,
		selectable = true
	}: Props = $props();

	const data: Row[] = [
		{ id: 'r1', name: 'Northwind order', owner: 'Mei Tan', amount: 1240, status: 'Open' },
		{
			id: 'r2',
			name: 'Contoso renewal with a long name that will not fit in its column',
			owner: 'Ana Silva',
			amount: 310,
			status: 'Won'
		},
		{ id: 'r3', name: 'Fabrikam trial', owner: 'Li Wei', amount: 9800, status: 'Open' }
	];
	let sort = $state<SortState>(null);
	let selected = $state(new Set<string>());
	let current = $state<string | null>(null);
	let opened = $state('');
	const rows = $derived(
		sort
			? [...data].sort((a, b) => {
					const key = sort!.column as keyof Row;
					const result = a[key] < b[key] ? -1 : a[key] > b[key] ? 1 : 0;
					return sort!.direction === 'asc' ? result : -result;
				})
			: data
	);
	const columns = $derived<Column<Row>[]>([
		{
			id: 'name',
			header: $t('ui.dataTable.sampleName'),
			width: 220,
			sortable: true,
			frozen: true,
			cell: nameCell
		},
		{
			id: 'owner',
			header: $t('ui.dataTable.sampleOwner'),
			width: 140,
			sortable: true,
			cell: ownerCell
		},
		{
			id: 'amount',
			header: $t('ui.dataTable.sampleAmount'),
			width: 120,
			align: 'end',
			sortable: true,
			cell: amountCell
		},
		{ id: 'status', header: $t('ui.dataTable.sampleStatus'), width: 120, cell: statusCell }
	]);
</script>

{#snippet nameCell(row: Row)}{row.name}{/snippet}
{#snippet ownerCell(row: Row)}{row.owner}{/snippet}
{#snippet amountCell(row: Row)}{row.amount}{/snippet}
{#snippet statusCell(row: Row)}{row.status}{/snippet}
{#snippet actions(row: Row)}
	<button type="button" class="act" aria-label="{$t('ui.dataTable.sampleRowAction')} {row.name}"
		>…</button
	>
{/snippet}
{#snippet footer()}{$t('ui.dataTable.sampleFooter')}{/snippet}

<div
	style:width={frame ? `${frame}px` : undefined}
	data-opened={opened}
	data-selected={[...selected].join(',')}
>
	<DataTable
		{rows}
		{columns}
		rowId={(row) => row.id}
		rowLabel={(row) => row.name}
		label={$t('ui.dataTable.sampleName')}
		state={tableState}
		stateAction={() => (opened = 'state-action')}
		{density}
		{selectable}
		bind:selected
		{sort}
		onsort={(next) => (sort = next)}
		currentId={current}
		rowHref={(row) => `#record-${row.id}`}
		onrowopen={(row) => {
			opened = row.id;
			current = row.id;
		}}
		onclosepanel={() => (current = null)}
		rowActions={actions}
		{footer}
		rowAttributes={(row) => ({ 'data-record-row': row.id })}
	/>
</div>

<style>
	.act {
		width: 24px;
		height: 24px;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
	}
</style>
