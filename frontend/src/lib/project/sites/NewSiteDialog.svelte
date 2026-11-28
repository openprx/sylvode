<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import Radio from '#lib/ui/radio/Radio.svelte';
	import RadioGroup from '#lib/ui/radio/RadioGroup.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import { isSlug, slugFromTitle, titleValue, validText } from './view';
	import type { Site } from './wire';

	/** The "New site" dialog: a blank site (title and address), or the way to the Market's templates. */
	let {
		workspaceId,
		projectId,
		onclose,
		oncreated
	}: {
		workspaceId: string;
		projectId: string;
		onclose: () => void;
		oncreated: (site: Site) => void;
	} = $props();

	let mode = $state<'blank' | 'template'>('blank');
	let titleEn = $state('');
	let titleZh = $state('');
	let slug = $state('');
	let slugEdited = $state(false);
	let busy = $state(false);
	let titleError = $state('');
	let slugError = $state('');
	let formError = $state('');

	function typed(value: string): void {
		titleEn = value;
		titleError = '';
		if (!slugEdited) slug = slugFromTitle(value);
	}

	function typedSlug(value: string): void {
		slug = value;
		slugEdited = true;
		slugError = '';
	}

	async function submit(): Promise<void> {
		titleError = '';
		slugError = '';
		formError = '';
		if (!validText(titleEn) || (titleZh.trim() !== '' && !validText(titleZh)))
			titleError = $t('sites.errors.title_invalid');
		if (!isSlug(slug)) slugError = $t('sites.errors.slug_invalid');
		if (titleError || slugError) return;
		busy = true;
		const result = await sitesApi.create(projectId, { slug, title: titleValue(titleEn, titleZh) });
		busy = false;
		if (result.code === 0 && result.data) {
			oncreated(result.data);
			return;
		}
		const message = errorText(result, $t);
		if (result.error_code === 'slug_taken' || result.error_code === 'slug_invalid')
			slugError = message;
		else if (result.error_code === 'title_invalid') titleError = message;
		else formError = message;
	}

	function openMarket(): void {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- full console path built here
		void goto(`/workspace/${encodeURIComponent(workspaceId)}/market`);
	}
</script>

<Dialog open title={$t('sites.create.title')} {onclose} testId="new-site-dialog">
	<form
		class="form"
		onsubmit={(event) => {
			event.preventDefault();
			if (mode === 'blank') void submit();
		}}
	>
		<RadioGroup label={$t('sites.create.kind')} bind:value={mode}>
			<Radio value="blank" label={$t('sites.create.blank')} />
			<Radio value="template" label={$t('sites.create.template')} />
		</RadioGroup>

		{#if mode === 'blank'}
			<TextField
				label={$t('sites.create.titleEn')}
				value={titleEn}
				oninput={(event) => typed(event.currentTarget.value)}
				error={titleError || undefined}
				maxlength={200}
				autocomplete="off"
			/>
			<TextField
				label={$t('sites.create.titleZh')}
				bind:value={titleZh}
				maxlength={200}
				autocomplete="off"
			/>
			<TextField
				label={$t('sites.create.slug')}
				value={slug}
				oninput={(event) => typedSlug(event.currentTarget.value)}
				helper={$t('sites.create.slugHelper')}
				error={slugError || undefined}
				maxlength={40}
				autocomplete="off"
			/>
			{#if formError}<TipBar tone="danger">{formError}</TipBar>{/if}
		{:else}
			<p class="note">{$t('sites.create.templateNote')}</p>
		{/if}
	</form>
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} onclick={onclose}>{$t('sites.cancel')}</Button>
		{#if mode === 'blank'}
			<Button
				variant="primary"
				loading={busy}
				onclick={() => void submit()}
				data-testid="create-site"
			>
				{$t('sites.create.submit')}
			</Button>
		{:else}
			<Button variant="primary" onclick={openMarket}>{$t('sites.create.openMarket')}</Button>
		{/if}
	{/snippet}
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.note {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
