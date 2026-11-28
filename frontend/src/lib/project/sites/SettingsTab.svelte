<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import {
		isSlug,
		originLines,
		settingsBody,
		settingsProblem,
		titleFields,
		titleValue,
		validText
	} from './view';
	import type { Site } from './wire';

	/**
	 * Title, address name, and the separate-origin deployment of a site. The title is for owners,
	 * admins and the site's editors; the address name (draft only) and the origin settings are for
	 * owners and admins. The server decides again on every save.
	 */
	let {
		site,
		isAdmin,
		canTitle,
		onchanged
	}: {
		site: Site;
		isAdmin: boolean;
		/** Owner, admin or this site's editor. */
		canTitle: boolean;
		onchanged: (site: Site) => void;
	} = $props();

	const initial = $derived(titleFields(site.title));
	let titleEn = $state('');
	let titleZh = $state('');
	let slug = $state('');
	let origin = $state('');
	let ancestors = $state('');
	let seen = $state('');
	let busy = $state<'title' | 'slug' | 'deploy' | null>(null);
	let titleError = $state('');
	let slugError = $state('');
	let originError = $state('');
	let ancestorsError = $state('');
	let formError = $state('');

	// The form follows the stored site: on load, and again after any save (which returns the site).
	$effect(() => {
		const key = JSON.stringify([site.title, site.slug, site.settings]);
		if (key === seen) return;
		seen = key;
		titleEn = initial.en;
		titleZh = initial.zh;
		slug = site.slug;
		origin = site.settings.allowed_origin ?? '';
		ancestors = (site.settings.frame_ancestors ?? []).join('\n');
	});

	const slugEditable = $derived(isAdmin && site.status === 'draft');
	const titleDirty = $derived(titleEn !== initial.en || titleZh !== initial.zh);
	const slugDirty = $derived(slug !== site.slug);
	const deployDirty = $derived(
		origin.trim() !== (site.settings.allowed_origin ?? '') ||
			originLines(ancestors).join('\n') !== (site.settings.frame_ancestors ?? []).join('\n')
	);

	function done(next: Site, message: string): void {
		onchanged(next);
		toast.success(message);
	}

	async function saveTitle(): Promise<void> {
		titleError = formError = '';
		if (!validText(titleEn) || (titleZh.trim() !== '' && !validText(titleZh))) {
			titleError = $t('sites.errors.title_invalid');
			return;
		}
		busy = 'title';
		const result = await sitesApi.update(site.id, { title: titleValue(titleEn, titleZh) });
		busy = null;
		if (result.code === 0 && result.data) done(result.data, $t('sites.settings.saved'));
		else if (result.error_code === 'title_invalid') titleError = errorText(result, $t);
		else formError = errorText(result, $t);
	}

	async function saveSlug(): Promise<void> {
		slugError = formError = '';
		if (!isSlug(slug)) {
			slugError = $t('sites.errors.slug_invalid');
			return;
		}
		busy = 'slug';
		const result = await sitesApi.update(site.id, { slug });
		busy = null;
		if (result.code === 0 && result.data) done(result.data, $t('sites.settings.saved'));
		else if (['slug_invalid', 'slug_taken', 'site_not_draft'].includes(result.error_code ?? ''))
			slugError = errorText(result, $t);
		else formError = errorText(result, $t);
	}

	async function saveDeploy(): Promise<void> {
		originError = ancestorsError = formError = '';
		const lines = originLines(ancestors);
		const problem = settingsProblem(origin, lines);
		if (problem === 'allowed_origin_invalid')
			originError = $t('sites.errors.allowed_origin_invalid');
		if (problem === 'frame_ancestors_invalid')
			ancestorsError = $t('sites.errors.frame_ancestors_invalid');
		if (problem) return;
		busy = 'deploy';
		const result = await sitesApi.putSettings(site.id, settingsBody(site.settings, origin, lines));
		busy = null;
		if (result.code === 0 && result.data) done(result.data, $t('sites.settings.saved'));
		else if (result.error_code === 'allowed_origin_invalid') originError = errorText(result, $t);
		else if (result.error_code === 'frame_ancestors_invalid')
			ancestorsError = errorText(result, $t);
		else formError = errorText(result, $t);
	}

	async function copyAddress(): Promise<void> {
		if (!site.url) return;
		try {
			await navigator.clipboard.writeText(site.url);
			toast.success($t('sites.settings.copied'));
		} catch {
			toast.error($t('sites.settings.copyFailed'));
		}
	}
</script>

<div class="settings" data-testid="site-settings" data-locale={$locale}>
	{#if formError}<TipBar tone="danger">{formError}</TipBar>{/if}

	<section class="group">
		<h2>{$t('sites.settings.titleHeading')}</h2>
		<TextField
			label={$t('sites.create.titleEn')}
			bind:value={titleEn}
			error={titleError || undefined}
			disabled={!canTitle}
			maxlength={200}
			autocomplete="off"
		/>
		<TextField
			label={$t('sites.create.titleZh')}
			bind:value={titleZh}
			disabled={!canTitle}
			maxlength={200}
			autocomplete="off"
		/>
		{#if canTitle}
			<div class="row">
				<Button
					variant="primary"
					loading={busy === 'title'}
					disabled={!titleDirty}
					onclick={() => void saveTitle()}
					data-testid="save-title"
				>
					{$t('sites.settings.save')}
				</Button>
			</div>
		{:else}
			<p class="note">{$t('sites.settings.titleReadOnly')}</p>
		{/if}
	</section>

	<section class="group">
		<h2>{$t('sites.settings.addressHeading')}</h2>
		<TextField
			label={$t('sites.create.slug')}
			bind:value={slug}
			helper={slugEditable
				? $t('sites.create.slugHelper')
				: site.status === 'draft'
					? $t('sites.settings.slugAdminOnly')
					: $t('sites.settings.slugDraftOnly')}
			error={slugError || undefined}
			disabled={!slugEditable}
			maxlength={40}
			autocomplete="off"
		/>
		{#if slugEditable}
			<div class="row">
				<Button
					variant="primary"
					loading={busy === 'slug'}
					disabled={!slugDirty}
					onclick={() => void saveSlug()}
					data-testid="save-slug"
				>
					{$t('sites.settings.saveSlug')}
				</Button>
			</div>
		{/if}
		<div class="url">
			<span class="label">{$t('sites.settings.siteAddress')}</span>
			{#if site.url}
				<code data-testid="site-url">{site.url}</code>
				<Button variant="secondary" size="sm" onclick={() => void copyAddress()}>
					{$t('sites.settings.copy')}
				</Button>
			{:else}
				<span class="note">{$t('sites.list.notServed')}</span>
			{/if}
		</div>
	</section>

	<section class="group">
		<h2>{$t('sites.settings.deployHeading')}</h2>
		<p class="note">{$t('sites.settings.deployLead')}</p>
		{#if site.url_mode === 'same_origin'}
			<TipBar tone="neutral" data-testid="same-origin-note">
				{$t('sites.settings.sameOriginNote')}
			</TipBar>
		{/if}
		<TextField
			label={$t('sites.settings.origin')}
			bind:value={origin}
			helper={$t('sites.settings.originHelper')}
			error={originError || undefined}
			disabled={!isAdmin}
			placeholder="https://help.example.com"
			autocomplete="off"
			inputmode="url"
		/>
		<Textarea
			label={$t('sites.settings.ancestors')}
			bind:value={ancestors}
			helper={$t('sites.settings.ancestorsHelper')}
			error={ancestorsError || undefined}
			disabled={!isAdmin}
			rows={3}
		/>
		{#if isAdmin}
			<div class="row">
				<Button
					variant="primary"
					loading={busy === 'deploy'}
					disabled={!deployDirty}
					onclick={() => void saveDeploy()}
					data-testid="save-deploy"
				>
					{$t('sites.settings.save')}
				</Button>
			</div>
		{:else}
			<p class="note">{$t('sites.settings.adminOnly')}</p>
		{/if}
	</section>
</div>

<style>
	.settings {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-8);
		max-width: 640px;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	h2 {
		margin: 0;
		font-size: var(--rf-size-title);
		font-weight: var(--rf-weight-semibold);
	}
	.note {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.row {
		display: flex;
		gap: var(--rf-space-4);
	}
	.url {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
	.label {
		font-weight: var(--rf-weight-semibold);
	}
	code {
		overflow-wrap: anywhere;
	}
</style>
