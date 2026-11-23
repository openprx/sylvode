<svelte:options runes />

<script lang="ts">
	import { untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { inboundApi } from '#lib/api/inbound.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import {
		changedFields,
		createBody,
		defaultDraft,
		draftOf,
		hasChanges,
		isValid,
		routeFailure,
		turnsSignatureOff,
		validateDraft,
		type SourceDraft,
		type SourceField
	} from './source-form';
	import type { SecretsAnswer, Source } from './wire';

	interface Props {
		/** `create` makes a new source; `edit` changes `source`. */
		source?: Source;
		projectId: string;
		oncancel: () => void;
		/** A created source: the answer holds the credentials, which are the caller's from here on. */
		oncreated?: (answer: SecretsAnswer) => void;
		onsaved?: (source: Source) => void;
		/** A 409: the source changed since it was read; the section offers to read the list again. */
		onstale?: () => void;
	}
	let { source, projectId, oncancel, oncreated, onsaved, onstale }: Props = $props();

	const editing = $derived(source !== undefined);
	let draft = $state<SourceDraft>(untrack(() => (source ? draftOf(source) : defaultDraft())));
	let advancedOpen = $state(false);
	let submitted = $state(false);
	let busy = $state(false);
	let serverError = $state<{ field: SourceField; text: string } | null>(null);
	let topError = $state<string | null>(null);
	let unsignedAsk = $state(false);

	const errors = $derived(validateDraft(draft));
	const change = $derived(source ? changedFields(draft, source) : {});
	const idle = $derived(source !== undefined && !hasChanges(change));
	const instanceId = $derived(source?.id ?? 'new');

	/** A message shows once the form was submitted or the server named the field. */
	function shown(field: SourceField): string | undefined {
		if (serverError?.field === field) return serverError.text;
		const key = errors[field];
		return submitted && key ? $t(key) : undefined;
	}

	async function submit(confirmedUnsigned = false): Promise<void> {
		submitted = true;
		serverError = null;
		topError = null;
		if (!isValid(errors)) {
			if (errors.rate || errors.cap || errors.allowlist) advancedOpen = true;
			return;
		}
		if (idle) return;
		if (source && turnsSignatureOff(change) && !confirmedUnsigned) {
			unsignedAsk = true;
			return;
		}
		busy = true;
		if (source) {
			const result = await inboundApi.updateSource(source.id, change);
			busy = false;
			if (result.ok) onsaved?.(result.value);
			else fail(result);
			return;
		}
		const result = await inboundApi.createSource(projectId, createBody(draft));
		busy = false;
		if (result.ok) oncreated?.(result.value);
		else fail(result);
	}

	function fail(result: { code: number; message: string; messageKey?: string }): void {
		const routed = routeFailure({ code: result.code, message: describeResult(result) });
		if (routed.type === 'field') {
			serverError = { field: routed.field, text: routed.message };
			if (routed.field !== 'name') advancedOpen = true;
		} else if (routed.type === 'stale') onstale?.();
		else topError = routed.message;
	}
</script>

<form
	class="form"
	data-testid={editing ? 'source-edit-form' : 'source-create-form'}
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	{#if topError}
		<TipBar tone="danger">{topError}</TipBar>
	{/if}

	<TextField
		label={$t('inbound.sources.name')}
		bind:value={draft.name}
		helper={$t('inbound.sources.nameHelp')}
		error={shown('name')}
		required
		autocomplete="off"
		disabled={busy}
	/>

	<Switch
		checked={draft.requireSignature}
		label={$t('inbound.sources.requireSignature')}
		disabled={busy}
		onchange={(event) => (draft.requireSignature = event.currentTarget.checked)}
	/>
	<p class="helper">{$t('inbound.sources.requireSignatureHelp')}</p>
	{#if !draft.requireSignature}
		<SecurityWarning tone="warning" title={$t('inbound.extra.unsignedTitle')}>
			{$t('inbound.sources.unsignedWarning')}
		</SecurityWarning>
	{/if}

	<div class="fold">
		<Button
			variant="text"
			aria-expanded={advancedOpen}
			aria-controls={`source-advanced-${instanceId}`}
			onclick={() => (advancedOpen = !advancedOpen)}
		>
			{$t('inbound.sources.advanced')}
		</Button>
		{#if advancedOpen}
			<div class="advanced" id={`source-advanced-${instanceId}`}>
				<TextField
					label={$t('inbound.sources.ratePerMinute')}
					type="number"
					inputmode="numeric"
					bind:value={draft.rate}
					error={shown('rate')}
					disabled={busy}
				/>
				<TextField
					label={$t('inbound.sources.dailyCap')}
					type="number"
					inputmode="numeric"
					bind:value={draft.cap}
					error={shown('cap')}
					disabled={busy}
				/>
				<div class="mono">
					<Textarea
						label={$t('inbound.sources.allowlist')}
						bind:value={draft.allowlist}
						rows={4}
						helper={$t('inbound.sources.allowlistHelp')}
						error={shown('allowlist')}
						spellcheck={false}
						autocomplete="off"
						disabled={busy}
					/>
				</div>
			</div>
		{/if}
	</div>

	<div class="actions">
		{#if editing}
			<Button
				type="submit"
				variant="primary"
				loading={busy}
				loadingLabel={$t('inbound.sources.saving')}
				disabled={idle}
				disabledReason={idle ? $t('inbound.sources.noChanges') : undefined}
			>
				{$t('inbound.sources.save')}
			</Button>
		{:else}
			<Button
				type="submit"
				variant="primary"
				loading={busy}
				loadingLabel={$t('inbound.sources.creating')}
			>
				{$t('inbound.sources.create')}
			</Button>
		{/if}
		<Button variant="secondary" disabled={busy} onclick={oncancel}>
			{$t('inbound.sources.cancel')}
		</Button>
	</div>
</form>

<Dialog
	open={unsignedAsk}
	title={$t('inbound.sources.unsignedDialog.title', { values: { source: source?.name ?? '' } })}
	role="alertdialog"
	onclose={() => (unsignedAsk = false)}
>
	<p>{$t('inbound.sources.unsignedDialog.body')}</p>
	{#snippet actions()}
		<Button variant="secondary" data-autofocus onclick={() => (unsignedAsk = false)}>
			{$t('inbound.sources.cancel')}
		</Button>
		<Button
			variant="primary"
			onclick={() => {
				unsignedAsk = false;
				void submit(true);
			}}
		>
			{$t('inbound.sources.unsignedDialog.confirm')}
		</Button>
	{/snippet}
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		max-width: 560px;
	}
	.helper {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.fold {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
	.advanced {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		width: 100%;
	}
	.mono :global(textarea) {
		font-family: var(--rf-font-mono-stack);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
</style>
