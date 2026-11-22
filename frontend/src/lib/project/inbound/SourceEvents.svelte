<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import X from '@lucide/svelte/icons/x';
	import { inboundApi } from '#lib/api/inbound.js';
	import { absoluteTime, relativeTime } from '#lib/project/agent-tasks/time.js';
	import Button from '#lib/ui/button/Button.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import EventDetailDialog from './EventDetailDialog.svelte';
	import SeverityChip from './SeverityChip.svelte';
	import { appendEvents, hasOlder, olderCursor } from './source-view';
	import { eventWorkItemHref, outcomeTone, outcomeWord } from './status';
	import type { InboundEvent, Rule, Source } from './wire';

	interface Props {
		source: Source;
		workspaceId: string;
		projectId: string;
		onclose: () => void;
	}
	let { source, workspaceId, projectId, onclose }: Props = $props();

	type Load = 'loading' | 'error' | 'ready';
	let load = $state<Load>('loading');
	let events = $state<InboundEvent[]>([]);
	let more = $state(false);
	let olderBusy = $state(false);
	let olderFailed = $state(false);
	let rules = $state<Rule[] | null>(null);
	let opened = $state<InboundEvent | null>(null);

	async function reload(): Promise<void> {
		load = 'loading';
		olderFailed = false;
		const result = await inboundApi.listEvents(source.id);
		if (!result.ok) {
			load = 'error';
			return;
		}
		events = result.value;
		more = hasOlder(result.value.length);
		load = 'ready';
	}

	async function older(): Promise<void> {
		const before = olderCursor(events);
		if (before === null) return;
		olderBusy = true;
		olderFailed = false;
		const result = await inboundApi.listEvents(source.id, { before });
		olderBusy = false;
		if (!result.ok) {
			olderFailed = true;
			return;
		}
		events = appendEvents(events, result.value);
		more = hasOlder(result.value.length);
	}

	onMount(() => {
		void reload();
		// Rule names for the detail dialog; without them the dialog shows the rule id.
		void inboundApi.listRules(projectId).then((result) => {
			if (result.ok) rules = result.value;
		});
	});

	const columns: Column<InboundEvent>[] = [
		{ id: 'received', header: '', width: 140, cell: receivedCell },
		{ id: 'kind', header: '', width: 200, cell: kindCell },
		{ id: 'severity', header: '', width: 110, cell: severityCell },
		{ id: 'outcome', header: '', width: 190, cell: outcomeCell },
		{ id: 'workItem', header: '', width: 120, cell: workItemCell },
		{ id: 'signed', header: '', width: 110, cell: signedCell }
	];
	const headers: Record<string, string> = {
		received: 'inbound.events.col.received',
		kind: 'inbound.events.col.kind',
		severity: 'inbound.events.col.severity',
		outcome: 'inbound.events.col.outcome',
		workItem: 'inbound.events.col.workItem',
		signed: 'inbound.events.col.signed'
	};
	const headed = $derived(columns.map((column) => ({ ...column, header: $t(headers[column.id]) })));
	const tableState = $derived(
		load === 'loading'
			? 'loading'
			: load === 'error'
				? 'failed'
				: events.length === 0
					? 'empty'
					: 'ready'
	);
</script>

{#snippet receivedCell(event: InboundEvent)}
	<span title={absoluteTime(event.received_at)}>{relativeTime(event.received_at)}</span>
{/snippet}
{#snippet kindCell(event: InboundEvent)}<span class="mono">{event.kind}</span>{/snippet}
{#snippet severityCell(event: InboundEvent)}<SeverityChip
		severity={event.severity_rank}
	/>{/snippet}
{#snippet outcomeCell(event: InboundEvent)}
	{@const word = outcomeWord(event.outcome)}
	<StatusChip tone={outcomeTone(event.outcome)}>
		{#if word.key !== null}{$t(word.key)}{:else}<span class="mono">{word.code}</span>{/if}
	</StatusChip>
{/snippet}
{#snippet workItemCell(event: InboundEvent)}
	{@const href = eventWorkItemHref(workspaceId, projectId, event)}
	{#if href && event.work_item}
		<!-- eslint-disable svelte/no-navigation-without-resolve -- full path built by the caller -->
		<a {href} class="link" onclick={(e) => e.stopPropagation()}>{event.work_item.identifier}</a>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{:else}
		—
	{/if}
{/snippet}
{#snippet signedCell(event: InboundEvent)}
	<StatusChip tone="neutral">
		{event.signed ? $t('inbound.events.signedYes') : $t('inbound.events.signedNo')}
	</StatusChip>
{/snippet}

<section class="events" data-testid="source-events">
	<header class="head">
		<h3 class="title">{$t('inbound.events.title', { values: { source: source.name } })}</h3>
		<IconButton
			icon={RefreshCw}
			label={$t('inbound.events.refresh')}
			disabled={load === 'loading'}
			onclick={() => void reload()}
		/>
		<IconButton icon={X} label={$t('ui.dialog.close')} onclick={onclose} />
	</header>

	{#if tableState === 'empty'}
		<StateBlock kind="empty" message={$t('inbound.events.empty')} />
	{:else}
		<DataTable
			rows={events}
			columns={headed}
			rowId={(event) => event.id}
			label={$t('inbound.events.title', { values: { source: source.name } })}
			state={tableState}
			stateAction={() => void reload()}
			thing={$t('inbound.events.thing')}
			onrowopen={(event) => (opened = event)}
		/>
	{/if}

	{#if load === 'ready' && more}
		<div class="more">
			<Button variant="secondary" loading={olderBusy} onclick={() => void older()}>
				{$t('inbound.events.older')}
			</Button>
			{#if olderFailed}
				<StateBlock kind="loadFailed" thing={$t('inbound.events.thing')} />
			{/if}
		</div>
	{/if}
</section>

<EventDetailDialog
	open={opened !== null}
	event={opened}
	sourceId={source.id}
	{workspaceId}
	{projectId}
	{rules}
	onclose={() => (opened = null)}
/>

<style>
	.events {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding-top: var(--rf-space-4);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
	}
	.title {
		flex: 1;
		margin: 0;
		font-size: var(--rf-size-body);
		font-weight: 600;
	}
	.mono {
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
	}
	.link {
		color: inherit;
		text-decoration: underline;
	}
	.more {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		align-items: flex-start;
	}
</style>
