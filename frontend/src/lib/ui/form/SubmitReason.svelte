<script lang="ts">
	import { t } from 'svelte-i18n';
	import { assembleReason, focusField, type MissingField, type Translate } from './field-state';

	interface Props {
		missing: readonly MissingField[];
		/** What the disabled button would do, as a verb phrase: "create the record". */
		action: string;
		/** Defaults to focusing the control whose id is the field id. */
		onfocusfield?: (id: string) => void;
		id?: string;
	}

	let { missing, action, onfocusfield = focusField, id }: Props = $props();

	const translate = $derived<Translate>((key, values) => $t(key, { values }));
	const parts = $derived(assembleReason(missing, action, translate));
</script>

{#if parts.length > 0}
	<span class="reason" {id} data-ui="submit-reason">
		{#each parts as part, index (index)}
			{#if part.kind === 'link'}
				<button type="button" class="link" onclick={() => onfocusfield(part.id)}>{part.text}</button
				>
			{:else}{part.text}{/if}
		{/each}
	</span>
{/if}

<style>
	.reason {
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
	.link {
		padding: 0;
		border: 0;
		border-radius: 2px;
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		text-decoration: underline;
		cursor: pointer;
	}
	.link:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
</style>
