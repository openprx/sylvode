<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Copy from '@lucide/svelte/icons/copy';
	import { copyText } from '#lib/clipboard.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import {
		canFinish,
		credentialsJson,
		guessedEndpoint,
		visibleValues,
		type SecretsState
	} from './secrets-state';

	interface Props {
		/** The state as the section holds it; this component keeps no copy of the values. */
		secrets: SecretsState;
		sourceName: string;
		requireSignature: boolean;
		onstored: (stored: boolean) => void;
		ondone: () => void;
	}
	let { secrets, sourceName, requireSignature, onstored, ondone }: Props = $props();

	const values = $derived(visibleValues(secrets));
	const finishable = $derived(canFinish(secrets));
	const origin = typeof window === 'undefined' ? '' : window.location.origin;
	const fallbackUrl = guessedEndpoint(origin);
	const endpointGuessed = $derived(values !== null && values.endpoint === null);

	/** The toast names the field, never the value. */
	async function copy(text: string, field: string): Promise<void> {
		if (await copyText(text)) toast.success($t('inbound.secrets.copied', { values: { field } }));
		else toast.error($t('inbound.secrets.copyFailed'));
	}
</script>

{#snippet copyButton(text: string, field: string)}
	<IconButton
		icon={Copy}
		label={`${$t('inbound.secrets.copy')}: ${field}`}
		onclick={() => void copy(text, field)}
	/>
{/snippet}

{#if values}
	<section class="secrets" aria-label={$t('inbound.secrets.title')} data-testid="source-secrets">
		<SecurityWarning tone="warning" title={$t('inbound.secrets.title')}>
			{$t('inbound.secrets.body', { values: { source: sourceName } })}
		</SecurityWarning>

		<div class="fields">
			<TextField
				label={$t('inbound.secrets.endpoint')}
				value={values.endpoint ?? fallbackUrl}
				helper={endpointGuessed ? $t('inbound.secrets.endpointGuess') : undefined}
				readonly
				spellcheck={false}
				autocomplete="off"
				data-testid="secret-endpoint"
			>
				{#snippet trailing()}
					{@render copyButton(values.endpoint ?? fallbackUrl, $t('inbound.secrets.endpoint'))}
				{/snippet}
			</TextField>
			<TextField
				label={$t('inbound.secrets.token')}
				value={values.token}
				readonly
				spellcheck={false}
				autocomplete="off"
				data-testid="secret-token"
			>
				{#snippet trailing()}
					{@render copyButton(values.token, $t('inbound.secrets.token'))}
				{/snippet}
			</TextField>
			<TextField
				label={$t('inbound.secrets.signingSecret')}
				value={values.signingSecret}
				helper={requireSignature
					? $t('inbound.secrets.signingHelp')
					: $t('inbound.secrets.signingOptional')}
				readonly
				spellcheck={false}
				autocomplete="off"
				data-testid="secret-signing"
			>
				{#snippet trailing()}
					{@render copyButton(values.signingSecret, $t('inbound.secrets.signingSecret'))}
				{/snippet}
			</TextField>
		</div>

		<div class="buttons">
			<Button
				variant="secondary"
				onclick={() => void copy(credentialsJson(values, fallbackUrl), 'JSON')}
			>
				{$t('inbound.secrets.copyAll')}
			</Button>
		</div>

		<Checkbox
			checked={secrets.phase === 'shown' && secrets.stored}
			label={$t('inbound.secrets.stored')}
			onchange={(event) => onstored(event.currentTarget.checked)}
		/>
		<div class="buttons">
			<Button
				variant="primary"
				disabled={!finishable}
				disabledReason={finishable ? undefined : $t('inbound.secrets.doneReason')}
				onclick={ondone}
			>
				{$t('inbound.secrets.done')}
			</Button>
		</div>
	</section>
{/if}

<style>
	.secrets {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.fields :global(input) {
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
	}
	.buttons {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
</style>
