<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import MenuItem from '../menu/MenuItem.svelte';
	import SidePanel from './SidePanel.svelte';
	import { defaultWidth, type PanelMode } from './side-panel';

	interface Props {
		id: string;
		/** CSS width of the container; decides docked or overlay. */
		containerWidth: number;
	}
	let { id, containerWidth }: Props = $props();

	let open = $state(false);
	let width = $state(defaultWidth(typeof window === 'undefined' ? 1280 : window.innerWidth));
	let record = $state(1);
	let mode = $state<PanelMode>('docked');
	let opener = $state<HTMLElement | null>(null);
</script>

<div
	class="frame"
	style:width="{containerWidth}px"
	data-width={width}
	data-mode={mode}
	data-record={record}
>
	<div class="list">
		<button
			bind:this={opener}
			type="button"
			class="row"
			data-testid="panel-opener-{id}"
			onclick={() => (open = true)}
		>
			{$t('ui.sidePanel.sampleRow')} 1
		</button>
		<button type="button" class="row" data-testid="panel-row-2-{id}" onclick={() => (open = true)}>
			{$t('ui.sidePanel.sampleRow')} 2
		</button>
	</div>
	{#if open}
		<SidePanel
			title={$t('ui.sidePanel.sampleTitle')}
			identifier={$t('ui.sidePanel.sampleIdentifier')}
			status="ready"
			{width}
			onresize={(px) => (width = px)}
			onclose={() => (open = false)}
			onfocuslist={() => opener?.focus()}
			onmodechange={(m) => (mode = m)}
			onprev={record > 1 ? () => (record -= 1) : undefined}
			prevDisabledReason={record > 1 ? undefined : $t('ui.sidePanel.firstRecord')}
			onnext={() => (record += 1)}
			fullPageHref="#full"
		>
			{#snippet chip()}<span>{$t('ui.sidePanel.sampleStatus')}</span>{/snippet}
			{#snippet menu()}<MenuItem compact>{$t('ui.sidePanel.sampleMenu')}</MenuItem>{/snippet}
			<dl class="props">
				<dt>{$t('ui.sidePanel.sampleLabel')}</dt>
				<dd>{$t('ui.sidePanel.sampleValue')}</dd>
			</dl>
			{#snippet footer()}
				<Button variant="primary">{$t('ui.sidePanel.sampleSave')}</Button>
			{/snippet}
		</SidePanel>
	{/if}
</div>

<style>
	.frame {
		position: relative;
		display: flex;
		height: 360px;
		border: var(--rf-border-width) solid var(--rf-line);
		background: var(--rf-bg);
	}
	.list {
		flex: 1;
		min-width: 0;
		padding: var(--rf-space-4);
	}
	.row {
		display: block;
		width: 100%;
		height: 32px;
		border: 0;
		background: transparent;
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		text-align: start;
	}
	.props {
		display: grid;
		grid-template-columns: 120px 1fr;
		margin: 0;
		font-size: var(--rf-size-body);
	}
	dt {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	dd {
		margin: 0;
	}
</style>
