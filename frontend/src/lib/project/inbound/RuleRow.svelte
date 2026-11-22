<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import Button from '#lib/ui/button/Button.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import Menu from '#lib/ui/menu/Menu.svelte';
	import MenuItem from '#lib/ui/menu/MenuItem.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import { kindsPreview } from './rule-form';
	import { severityKey } from './status';
	import type { Rule } from './wire';

	interface Props {
		rule: Rule;
		/** Zero-based place in the list. */
		index: number;
		count: number;
		/** The bound source's name; `null` for "any source". */
		sourceName: string | null;
		/** The handed-to bot's name when the action hands to a bot. */
		botName: string | null;
		warnBotNotConnected: boolean;
		warnNoRecipients: boolean;
		/** A write of the list is under way: the controls wait. */
		busy: boolean;
		/** Resolves to whether the write was accepted, so a refused switch snaps back. */
		ontoggle: (enabled: boolean) => Promise<boolean>;
		onmove: (delta: -1 | 1) => void;
		onedit: () => void;
		ondelete: () => void;
	}
	let {
		rule,
		index,
		count,
		sourceName,
		botName,
		warnBotNotConnected,
		warnNoRecipients,
		busy,
		ontoggle,
		onmove,
		onedit,
		ondelete
	}: Props = $props();

	// Follows the rule; the switch writes it while the person flips it, and a refusal snaps it back.
	let shown = $derived(rule.enabled);
	async function flip() {
		const ok = await ontoggle(shown);
		if (!ok) shown = rule.enabled;
	}

	let menuOpen = $state(false);
	let menuAnchor = $state<HTMLElement | null>(null);

	const kinds = $derived(kindsPreview(rule.kind_patterns));
	const botLabel = $derived(botName ?? $t('inbound.rules.unknownBot'));
</script>

<li class="row" data-testid="rule-row" data-rule-id={rule.id}>
	<div class="lines">
		<div class="line1">
			<span class="position">{index + 1}</span>
			<span class="name">{rule.name}</span>
			<StatusChip tone={rule.enabled ? 'success' : 'neutral'}>
				{$t(rule.enabled ? 'inbound.rules.enabled' : 'inbound.rules.disabled')}
			</StatusChip>
		</div>
		<div class="line2">
			<span>{sourceName ?? $t('inbound.rules.anySource')}</span>
			<span aria-hidden="true">·</span>
			{#if kinds.shown.length === 0}
				<span>{$t('inbound.rules.anyKind')}</span>
			{:else}
				<span class="mono">{kinds.shown.join(', ')}</span>
				{#if kinds.more > 0}
					<span>{$t('inbound.rules.moreKinds', { values: { count: kinds.more } })}</span>
				{/if}
			{/if}
			<span aria-hidden="true">·</span>
			<span
				>{$t('inbound.rules.andAbove', {
					values: { severity: $t(severityKey(rule.min_severity)) }
				})}</span
			>
			<span aria-hidden="true">·</span>
			<span>
				{rule.action === 'work_item_and_task'
					? $t('inbound.rules.actionBot', { values: { bot: botLabel } })
					: $t('inbound.rules.actionWorkItem')}
			</span>
		</div>
		{#if warnBotNotConnected}
			<div class="warning" data-testid="rule-warn-bot">
				{$t('inbound.rules.warnBotNotConnected', { values: { bot: botLabel } })}
			</div>
		{/if}
		{#if warnNoRecipients}
			<div class="warning" data-testid="rule-warn-recipients">
				{$t('inbound.rules.warnNoRecipients')}
			</div>
		{/if}
	</div>
	<div class="actions">
		<Switch
			bind:checked={shown}
			disabled={busy}
			aria-label={$t('inbound.rules.toggleLabel', { values: { name: rule.name } })}
			onchange={() => void flip()}
		/>
		<IconButton
			icon={ArrowUp}
			label={index === 0 ? $t('inbound.rules.first') : $t('inbound.rules.moveUp')}
			disabled={busy || index === 0}
			onclick={() => onmove(-1)}
		/>
		<IconButton
			icon={ArrowDown}
			label={index === count - 1 ? $t('inbound.rules.last') : $t('inbound.rules.moveDown')}
			disabled={busy || index === count - 1}
			onclick={() => onmove(1)}
		/>
		<Button size="sm" disabled={busy} onclick={onedit}>{$t('inbound.rules.edit')}</Button>
		<span class="menu-anchor" bind:this={menuAnchor}>
			<IconButton
				icon={Ellipsis}
				label={$t('inbound.rules.moreActions')}
				disabled={busy}
				aria-haspopup="menu"
				aria-expanded={menuOpen}
				onclick={() => (menuOpen = !menuOpen)}
			/>
		</span>
		<Menu
			open={menuOpen}
			anchor={menuAnchor}
			onclose={() => (menuOpen = false)}
			label={$t('inbound.rules.moreActions')}
			placement="bottom-end"
		>
			<MenuItem icon={Trash2} destructive onselect={ondelete}>
				{$t('inbound.rules.delete')}
			</MenuItem>
		</Menu>
	</div>
</li>

<style>
	.row {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--rf-space-4);
		min-height: 56px;
		padding: var(--rf-space-3) var(--rf-space-4);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		list-style: none;
	}
	.row:last-child {
		border-bottom: 0;
	}
	.lines {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
		min-width: 0;
	}
	.line1 {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		min-width: 0;
	}
	.position {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		min-width: 1.5em;
	}
	.name {
		font-size: var(--rf-size-body);
		font-weight: 600;
		overflow-wrap: anywhere;
	}
	.line2 {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.mono {
		font-family: var(--rf-font-mono-stack);
	}
	.warning {
		color: var(--rf-warning);
		font-size: var(--rf-size-secondary);
	}
	.actions {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--rf-space-2);
		flex: none;
	}
	.menu-anchor {
		display: inline-flex;
	}
	@media (max-width: 640px) {
		.row {
			flex-direction: column;
		}
		.actions {
			justify-content: flex-start;
		}
	}
</style>
