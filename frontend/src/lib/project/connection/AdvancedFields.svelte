<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import Radio from '#lib/ui/radio/Radio.svelte';
	import RadioGroup from '#lib/ui/radio/RadioGroup.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import type { Draft, DraftErrors, OnComplete } from './advanced';
	import type { Choice } from './options';
	import { blockReason, type TmuxBlock } from './tmux';
	import { vetoAllowed } from './validate';
	import { CHANNELS, LEVELS, type Channel, type Level } from './wire';

	interface Props {
		draft: Draft;
		errors: DraftErrors;
		states: Choice[];
		labels: Choice[];
		domains: Choice[];
		/** Distinguishes the ids of two instances on one page. */
		scope?: string;
		disabled?: boolean;
		/** Why the tmux channel cannot be chosen for this connection's CLI; `null` when it can. */
		tmuxBlock?: TmuxBlock | null;
		/** The server's refusal of the channel, shown under the channel control. */
		channelError?: string;
	}
	let {
		draft = $bindable(),
		errors,
		states,
		labels,
		domains,
		scope = 'adv',
		disabled = false,
		tmuxBlock = null,
		channelError = undefined
	}: Props = $props();

	const channels = $derived(
		CHANNELS.map((value) => ({
			value,
			label: $t(`execution.connection.advanced.channel.${value}`),
			// The chosen option stays selectable so a saved tmux connection reads as it is.
			disabled: value === 'tmux' && tmuxBlock !== null && draft.channel !== 'tmux'
		}))
	);
	const tmuxReason = $derived(
		tmuxBlock !== null && draft.channel !== 'tmux'
			? blockReason(tmuxBlock, (key, values) => $t(key, { values }))
			: undefined
	);
	const levels = $derived(
		LEVELS.map((value) => ({ value, label: $t(`execution.connection.advanced.level.${value}`) }))
	);
	const vetoOk = $derived(vetoAllowed(draft.level));
	/** Domains not yet used by another row, plus the row's own. */
	const freeDomains = (own: string | null) =>
		domains.filter(
			(domain) => domain.value === own || !draft.domains.some((row) => row.domain === domain.value)
		);

	function addDomain() {
		draft.domains = [...draft.domains, { domain: null, level: draft.level }];
	}
	function removeDomain(index: number) {
		draft.domains = draft.domains.filter((_, at) => at !== index);
	}
	function setLevel(value: Level | null) {
		draft.level = value;
		if (!vetoAllowed(value)) draft.veto = false;
	}
</script>

<div class="advanced" data-testid="advanced-fields">
	<h3 class="group">{$t('execution.connection.advanced.execution')}</h3>

	<div class="channel">
		<span class="caption">{$t('execution.connection.advanced.channel.label')}</span>
		<Segmented
			id={`${scope}-channel`}
			label={$t('execution.connection.advanced.channel.label')}
			options={channels}
			value={draft.channel}
			{disabled}
			onchange={(value: Channel) => (draft.channel = value)}
			width="fill"
		/>
	</div>
	{#if tmuxReason}
		<p class="hint" data-testid="tmux-disabled-reason">{tmuxReason}</p>
	{/if}
	{#if channelError}
		<p class="error" role="alert" data-testid="channel-error">{channelError}</p>
	{/if}
	{#if draft.channel === 'tmux'}
		<SecurityWarning tone="warning" title={$t('execution.connection.advanced.tmuxWarningTitle')}>
			{$t('execution.connection.advanced.tmuxWarningBody')}
		</SecurityWarning>
	{/if}

	<RadioGroup
		label={$t('execution.connection.advanced.onComplete.label')}
		value={draft.onComplete}
		{disabled}
		onchange={(value) => (draft.onComplete = value as OnComplete)}
	>
		<Radio value="auto" label={$t('execution.connection.advanced.onComplete.auto')} />
		<Radio value="none" label={$t('execution.connection.advanced.onComplete.none')} />
		<Radio value="state" label={$t('execution.connection.advanced.onComplete.state')} />
	</RadioGroup>
	{#if draft.onComplete === 'state'}
		<Select
			label={$t('execution.connection.advanced.onComplete.state')}
			value={draft.stateKey}
			options={states}
			placeholder={$t('execution.connection.advanced.onComplete.stateChoose')}
			error={errors.state ? $t(errors.state) : undefined}
			{disabled}
			onchange={(value) => (draft.stateKey = value)}
		/>
	{/if}

	<Select
		label={$t('execution.connection.advanced.failLabel')}
		value={draft.failLabel}
		options={labels}
		allowEmpty
		{disabled}
		onchange={(value) => (draft.failLabel = value)}
	/>

	<h3 class="group">{$t('execution.connection.advanced.governance')}</h3>

	<Select
		label={$t('execution.connection.advanced.level.label')}
		value={draft.level}
		options={levels}
		placeholder={$t('execution.connection.advanced.level.placeholder')}
		error={errors.level ? $t(errors.level) : undefined}
		{disabled}
		onchange={setLevel}
	/>
	<div class="switch">
		<Switch
			checked={draft.veto}
			label={$t('execution.connection.advanced.veto')}
			disabled={disabled || !vetoOk}
			onchange={(event) => (draft.veto = event.currentTarget.checked)}
		/>
		{#if !vetoOk}
			<span class="hint">{$t('execution.connection.advanced.vetoReason')}</span>
		{/if}
		{#if errors.veto}<span class="error" role="alert">{$t(errors.veto)}</span>{/if}
	</div>
	<TextField
		label={$t('execution.connection.advanced.reasonLength')}
		type="number"
		inputmode="numeric"
		min="0"
		max="2000"
		bind:value={draft.reasonLength}
		helper={$t('execution.connection.advanced.reasonLengthHelp')}
		error={errors.reasonLength ? $t(errors.reasonLength) : undefined}
		{disabled}
	/>

	<div class="domains">
		<span class="caption">{$t('execution.connection.advanced.domains')}</span>
		<span class="hint">{$t('execution.connection.advanced.domainsHelp')}</span>
		{#each draft.domains as row, index (index)}
			<div class="row">
				<Select
					label={$t('execution.connection.advanced.domains')}
					labelHidden
					value={row.domain}
					options={freeDomains(row.domain)}
					placeholder={$t('execution.connection.advanced.domainPlaceholder')}
					error={errors.domains[index] ? $t(errors.domains[index] ?? '') : undefined}
					{disabled}
					onchange={(value) => (row.domain = value)}
				/>
				<Select
					label={$t('execution.connection.advanced.level.label')}
					labelHidden
					value={row.level}
					options={levels}
					placeholder={$t('execution.connection.advanced.level.placeholder')}
					{disabled}
					onchange={(value) => (row.level = value)}
				/>
				<Button variant="text" {disabled} onclick={() => removeDomain(index)}>
					{$t('execution.connection.advanced.domainRemove')}
				</Button>
			</div>
		{/each}
		<div>
			<Button
				variant="secondary"
				disabled={disabled || freeDomains(null).length === 0}
				disabledReason={freeDomains(null).length === 0
					? $t('execution.connection.advanced.domainNone')
					: undefined}
				onclick={addDomain}>{$t('execution.connection.advanced.domainAdd')}</Button
			>
		</div>
	</div>
</div>

<style>
	.advanced {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.group {
		margin: var(--rf-space-5) 0 0;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-medium);
		color: var(--rf-text);
	}
	.hint {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.error {
		color: var(--rf-danger);
		font-size: var(--rf-size-meta);
	}
	.caption {
		font-size: var(--rf-size-secondary);
		color: var(--rf-text);
	}
	.switch,
	.channel,
	.domains {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
	}
	.row {
		display: grid;
		grid-template-columns: 1fr 1fr auto;
		gap: var(--rf-space-4);
		align-items: end;
	}
</style>
