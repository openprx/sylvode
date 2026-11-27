<svelte:options runes />

<script lang="ts">
	/**
	 * The messages under one record editor field: the server's refusal of the last save
	 * (`validation_failed`) and the live rule hints of the draft, both rendered by code.
	 */
	import { locale, t } from 'svelte-i18n';
	import type { FormField } from '#lib/api/forms.js';
	import { errorMessage, type ErrorCode } from '#lib/forms/field-errors.js';

	interface Props {
		field: FormField;
		errors?: ErrorCode[];
		hints?: ErrorCode[];
	}

	let { field, errors = [], hints = [] }: Props = $props();

	const lang = $derived($locale === 'en' ? 'en' : 'zh');
	const errorCodes = $derived(new Set(errors.map((error) => `${error.code}:${error.rule ?? ''}`)));
	const extraHints = $derived(
		hints.filter((hint) => !errorCodes.has(`${hint.code}:${hint.rule ?? ''}`))
	);
</script>

{#if errors.length > 0}
	<ul class="space-y-0.5 text-xs text-danger" role="alert" data-field-errors={field.key}>
		{#each errors as error, index (index)}
			<li data-field-error-code={error.code}>{errorMessage(field, error, $t, lang)}</li>
		{/each}
	</ul>
{/if}
{#if extraHints.length > 0}
	<ul class="space-y-0.5 text-xs text-warning" aria-live="polite" data-field-hints={field.key}>
		{#each extraHints as hint, index (index)}
			<li data-field-hint-code={hint.code}>{errorMessage(field, hint, $t, lang)}</li>
		{/each}
	</ul>
{/if}
