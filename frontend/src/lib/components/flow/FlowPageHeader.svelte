<script lang="ts">
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import { authStore } from '#lib/stores/auth.js';
	import { get } from 'svelte/store';
	import { FlowMutationIntents } from '#lib/flow/social-actions.js';
	import {
		appearanceBlobUrl,
		flowApi,
		publicAppearanceUrl,
		type FlowAppearance,
		type FlowObjectView
	} from '#lib/api/flow.js';
	import { COVER_GALLERIES, PAGE_EMOJIS, galleryBackground } from '#lib/flow/appearance.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';
	import FlowObjectIcon from './FlowObjectIcon.svelte';

	interface Crumb {
		id: string;
		title: string;
		appearance?: FlowAppearance;
	}

	interface Props {
		scopeId: string;
		objectId: string;
		title: string;
		readOnly: boolean;
		appearance?: FlowAppearance | null;
		/** Public page: images load through the share token and nothing is editable. */
		publicToken?: string | null;
		onTitle?: (value: string) => void;
		onAppearance?: (appearance: FlowAppearance) => void;
	}

	let {
		scopeId,
		objectId,
		title,
		readOnly,
		appearance = null,
		publicToken = null,
		onTitle,
		onAppearance
	}: Props = $props();

	let titleInput = $state<HTMLTextAreaElement | null>(null);
	function resizeTitle(): void {
		if (!titleInput) return;
		titleInput.style.height = 'auto';
		titleInput.style.height = `${titleInput.scrollHeight}px`;
	}
	$effect(() => {
		void title;
		resizeTitle();
	});

	let crumbs = $state<Crumb[]>([]);
	let menu = $state<'icon' | 'cover' | null>(null);
	let failureKey = $state<string | null>(null);
	let coverUrl = $state<string | null>(null);
	let dragging = $state(false);
	let draftPosition = $state<number | null>(null);

	const cover = $derived(appearance?.cover ?? null);
	const position = $derived(draftPosition ?? cover?.position ?? 0.5);
	const gallery = $derived(cover?.kind === 'gallery' ? galleryBackground(cover.gallery_id) : null);

	$effect(() => {
		const id = objectId;
		let active = true;
		crumbs = [];
		void loadCrumbs(id).then((next) => {
			if (active) crumbs = next;
		});
		return () => {
			active = false;
		};
	});

	$effect(() => {
		const token = publicToken;
		const asset = cover?.kind === 'upload' ? cover.asset_id : null;
		coverUrl = null;
		if (!asset) return;
		if (token) {
			coverUrl = publicAppearanceUrl(token, asset);
			return;
		}
		let active = true;
		void appearanceBlobUrl(objectId, asset).then((url) => {
			if (active) coverUrl = url;
		});
		return () => {
			active = false;
		};
	});

	async function loadCrumbs(startId: string): Promise<Crumb[]> {
		if (publicToken) return [];
		const chain: Crumb[] = [];
		let current = startId;
		for (let depth = 0; depth < 32; depth += 1) {
			const result = await flowApi.getObject(current);
			if (result.code !== 0 || !result.data) break;
			const object: FlowObjectView = result.data;
			if (object.id !== startId && object.object_type !== 'navigator') {
				chain.unshift({
					id: object.id,
					title: object.title,
					appearance: object.appearance
				});
			}
			if (!object.parent_id || object.object_type === 'navigator') break;
			current = object.parent_id;
		}
		return chain;
	}

	function withAppearance(patch: FlowAppearance): FlowAppearance {
		return { ...(appearance ?? {}), ...patch };
	}

	const appearanceIntents = new FlowMutationIntents();
	function appearanceOperation(kind: string) {
		return `${get(authStore).user?.id ?? ''}:${objectId}:${kind}`;
	}

	async function save(next: FlowAppearance): Promise<void> {
		if (readOnly) return;
		failureKey = null;
		const actor = get(authStore).user?.id,
			target = objectId;
		const op = appearanceOperation('appearance');
		const result = await flowApi.setAppearance(target, next, appearanceIntents.key(op, next));
		if (actor !== get(authStore).user?.id || target !== objectId) return;
		if (result.code !== 0 || !result.data) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		appearanceIntents.complete(op);
		onAppearance?.(result.data);
		menu = null;
	}

	async function onUpload(event: Event, kind: 'icon' | 'cover'): Promise<void> {
		if (readOnly) return;
		const actor = get(authStore).user?.id,
			target = objectId;
		const input = event.currentTarget;
		if (!(input instanceof HTMLInputElement)) return;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		const data = await file.arrayBuffer();
		const bytes = new Uint8Array(data);
		let binary = '';
		for (const byte of bytes) binary += String.fromCharCode(byte);
		if (readOnly || actor !== get(authStore).user?.id || target !== objectId) return;
		const encoded = btoa(binary),
			op = appearanceOperation(`upload:${kind}`);
		const result = await flowApi.uploadAppearance(
			objectId,
			encoded,
			appearanceIntents.key(op, encoded)
		);
		if (actor !== get(authStore).user?.id || target !== objectId) return;
		if (result.code !== 0 || !result.data) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		appearanceIntents.complete(op);
		if (kind === 'icon') {
			await save(withAppearance({ icon: { kind: 'image', asset_id: result.data.asset_id } }));
		} else {
			await save(
				withAppearance({
					cover: { kind: 'upload', asset_id: result.data.asset_id, position }
				})
			);
		}
	}

	function onCoverPointer(event: PointerEvent, commit: boolean): void {
		const target = event.currentTarget;
		if (!(target instanceof HTMLElement)) return;
		const rect = target.getBoundingClientRect();
		const next = Math.min(1, Math.max(0, (event.clientY - rect.top) / Math.max(rect.height, 1)));
		draftPosition = next;
		dragging = !commit;
		if (commit && cover) {
			draftPosition = null;
			void save(withAppearance({ cover: { ...cover, position: next } }));
		}
	}
</script>

<svelte:window onresize={resizeTitle} />

<header
	class="flow-page-header bg-white dark:bg-slate-950"
	data-testid="flow-page-header"
	data-scope-id={scopeId}
>
	{#if cover}
		<div
			class="relative h-48 w-full overflow-hidden"
			data-testid="flow-page-cover"
			role="slider"
			tabindex={readOnly ? -1 : 0}
			aria-label={$t('flow.personal.coverPosition')}
			aria-valuemin={0}
			aria-valuemax={100}
			aria-valuenow={Math.round(position * 100)}
			aria-readonly={readOnly}
			onkeydown={(event) => {
				if (readOnly || !cover) return;
				const delta =
					event.key === 'ArrowUp' || event.key === 'ArrowRight'
						? 0.05
						: event.key === 'ArrowDown' || event.key === 'ArrowLeft'
							? -0.05
							: null;
				if (delta === null && event.key !== 'Home' && event.key !== 'End') return;
				event.preventDefault();
				const next =
					event.key === 'Home'
						? 0
						: event.key === 'End'
							? 1
							: Math.min(1, Math.max(0, position + (delta ?? 0)));
				void save(withAppearance({ cover: { ...cover, position: next } }));
			}}
			onpointerdown={(event) => {
				if (readOnly) return;
				dragging = true;
				event.currentTarget.setPointerCapture(event.pointerId);
				onCoverPointer(event, false);
			}}
			onpointermove={(event) => {
				if (!dragging || readOnly) return;
				onCoverPointer(event, false);
			}}
			onpointerup={(event) => {
				if (!dragging || readOnly) return;
				onCoverPointer(event, true);
			}}
		>
			{#if gallery}
				<div
					class="h-[200%] w-full"
					style:background={gallery}
					style:transform={`translateY(-${position * 50}%)`}
				></div>
			{:else if coverUrl}
				<img
					src={coverUrl}
					alt=""
					class="h-[200%] w-full object-cover"
					style:object-position={`center ${position * 100}%`}
					style:transform={`translateY(-${position * 50}%)`}
				/>
			{/if}
			{#if !readOnly}
				<p class="absolute right-4 bottom-3 rounded bg-black/40 px-2 py-1 text-xs text-white">
					{$t('flow.header.reposition')}
				</p>
			{/if}
		</div>
	{/if}

	<div class="mx-auto w-full max-w-3xl px-6 pt-6 sm:px-8 2xl:px-16">
		{#if crumbs.length > 0}
			<nav
				class="mb-3 flex flex-wrap items-center gap-1 text-sm text-slate-500"
				aria-label={$t('flow.header.breadcrumbs')}
			>
				{#each crumbs as crumb, index (crumb.id)}
					{#if index > 0}<span aria-hidden="true">/</span>{/if}
					<a
						class="inline-flex items-center gap-1 hover:underline"
						href={resolve('/(app)/flow/[objectId]', { objectId: crumb.id })}
					>
						<FlowObjectIcon appearance={crumb.appearance} objectId={crumb.id} />
						<span>{crumb.title || $t('flow.nav.untitled')}</span>
					</a>
				{/each}
			</nav>
		{/if}

		<div class="relative mb-2">
			{#if appearance?.icon}
				<FlowObjectIcon {appearance} {objectId} size="lg" />
			{/if}
			{#if !readOnly}
				<button
					type="button"
					class="appearance-action mt-1 text-sm text-slate-500 hover:text-slate-800 dark:hover:text-slate-200"
					onclick={() => (menu = menu === 'icon' ? null : 'icon')}
				>
					{appearance?.icon ? $t('flow.header.changeIcon') : $t('flow.header.addIcon')}
				</button>
			{/if}
			{#if menu === 'icon' && !readOnly}
				<div
					class="absolute z-20 mt-1 w-72 rounded-md border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-900"
				>
					<div class="grid grid-cols-8 gap-1">
						{#each PAGE_EMOJIS as emoji (emoji)}
							<button
								type="button"
								class="rounded p-1 text-xl hover:bg-slate-100 dark:hover:bg-slate-800"
								onclick={() => save(withAppearance({ icon: { kind: 'emoji', emoji } }))}
							>
								{emoji}
							</button>
						{/each}
					</div>
					<label class="mt-2 block text-xs text-slate-500">
						{$t('flow.header.uploadIcon')}
						<input
							class="mt-1 block w-full text-sm"
							type="file"
							accept="image/png,image/jpeg,image/webp,image/gif"
							onchange={(event) => onUpload(event, 'icon')}
						/>
					</label>
					{#if appearance?.icon}
						<button
							type="button"
							class="mt-2 text-xs text-slate-500"
							onclick={() => save(withAppearance({ icon: null }))}
						>
							{$t('flow.header.removeIcon')}
						</button>
					{/if}
				</div>
			{/if}
		</div>

		<div class="flex flex-col items-start gap-2">
			<textarea
				bind:this={titleInput}
				value={title}
				rows={1}
				oninput={(event) => {
					const target = event.currentTarget;
					const value = target.value.replace(/[\r\n]+/g, ' ');
					if (target.value !== value) target.value = value;
					onTitle?.(value);
					resizeTitle();
				}}
				onkeydown={(event) => {
					if (event.key === 'Enter' && !event.isComposing) {
						event.preventDefault();
						event.currentTarget.blur();
					}
				}}
				readonly={readOnly}
				placeholder={$t('flow.titlePlaceholder')}
				aria-label={$t('flow.personal.pageTitle')}
				class="w-full min-w-0 resize-none overflow-hidden border-none bg-transparent text-4xl leading-tight font-semibold text-slate-900 outline-none dark:text-slate-100"
			></textarea>
		</div>

		{#if !readOnly}
			<div class="relative mt-2">
				<button
					type="button"
					class="appearance-action text-sm text-slate-500 hover:text-slate-800"
					onclick={() => (menu = menu === 'cover' ? null : 'cover')}
				>
					{cover ? $t('flow.header.changeCover') : $t('flow.header.addCover')}
				</button>
				{#if menu === 'cover'}
					<div
						class="absolute z-20 mt-1 w-80 rounded-md border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-900"
					>
						<p class="mb-2 text-xs text-slate-500">{$t('flow.header.gallery')}</p>
						<div class="grid grid-cols-3 gap-2">
							{#each COVER_GALLERIES as entry (entry.id)}
								<button
									type="button"
									class="h-12 rounded"
									style:background={entry.background}
									aria-label={$t('flow.header.gallery')}
									onclick={() =>
										save(
											withAppearance({
												cover: { kind: 'gallery', gallery_id: entry.id, position: 0.5 }
											})
										)}
								></button>
							{/each}
						</div>
						<label class="mt-2 block text-xs text-slate-500">
							{$t('flow.header.uploadCover')}
							<input
								class="mt-1 block w-full text-sm"
								type="file"
								accept="image/png,image/jpeg,image/webp,image/gif"
								onchange={(event) => onUpload(event, 'cover')}
							/>
						</label>
						{#if cover}
							<button
								type="button"
								class="mt-2 text-xs text-slate-500"
								onclick={() => save(withAppearance({ cover: null }))}
							>
								{$t('flow.header.removeCover')}
							</button>
						{/if}
					</div>
				{/if}
			</div>
		{/if}
		{#if failureKey}
			<p class="mt-2 text-sm text-red-600" role="alert">{$t(failureKey)}</p>
		{/if}
	</div>
</header>

<style>
	@media (hover: hover) and (pointer: fine) {
		.appearance-action {
			opacity: 0;
			transition: opacity 0.12s;
		}
		.flow-page-header:hover .appearance-action,
		.flow-page-header:focus-within .appearance-action {
			opacity: 1;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.appearance-action {
			transition: none;
		}
	}
</style>
