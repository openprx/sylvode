<script lang="ts">
	import { t } from 'svelte-i18n';
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
	import FlowSyncIndicator from './FlowSyncIndicator.svelte';
	import type { SyncState } from '#lib/flow/types.js';

	interface Crumb {
		id: string;
		title: string;
		appearance?: FlowAppearance;
	}

	interface Props {
		workspaceId: string;
		objectId: string;
		title: string;
		readOnly: boolean;
		appearance?: FlowAppearance | null;
		syncState?: SyncState;
		pending?: number;
		/** Public page: images load through the share token and nothing is editable. */
		publicToken?: string | null;
		onTitle?: (value: string) => void;
		onAppearance?: (appearance: FlowAppearance) => void;
	}

	let {
		workspaceId,
		objectId,
		title,
		readOnly,
		appearance = null,
		syncState = 'local',
		pending = 0,
		publicToken = null,
		onTitle,
		onAppearance
	}: Props = $props();

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

	async function save(next: FlowAppearance): Promise<void> {
		failureKey = null;
		const result = await flowApi.setAppearance(objectId, next);
		if (result.code !== 0 || !result.data) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		onAppearance?.(result.data);
		menu = null;
	}

	async function onUpload(event: Event, kind: 'icon' | 'cover'): Promise<void> {
		const input = event.currentTarget;
		if (!(input instanceof HTMLInputElement)) return;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		const data = await file.arrayBuffer();
		const bytes = new Uint8Array(data);
		let binary = '';
		for (const byte of bytes) binary += String.fromCharCode(byte);
		const result = await flowApi.uploadAppearance(objectId, btoa(binary));
		if (result.code !== 0 || !result.data) {
			failureKey = flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
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

<header class="bg-white dark:bg-slate-950" data-testid="flow-page-header">
	{#if cover}
		<div
			class="relative h-48 w-full overflow-hidden"
			data-testid="flow-page-cover"
			onpointerdown={(event) => {
				if (readOnly) return;
				dragging = true;
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
				<div class="h-[200%] w-full" style:background={gallery} style:transform={`translateY(-${position * 50}%)`}></div>
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

	<div class="mx-auto w-full max-w-3xl px-16 pt-6">
		{#if crumbs.length > 0}
			<nav class="mb-3 flex flex-wrap items-center gap-1 text-sm text-slate-500" aria-label={$t('flow.header.breadcrumbs')}>
				{#each crumbs as crumb, index (crumb.id)}
					{#if index > 0}<span aria-hidden="true">/</span>{/if}
					<a class="inline-flex items-center gap-1 hover:underline" href={`/workspace/${workspaceId}/flow/${crumb.id}`}>
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
					class="mt-1 text-sm text-slate-500 hover:text-slate-800 dark:hover:text-slate-200"
					onclick={() => (menu = menu === 'icon' ? null : 'icon')}
				>
					{appearance?.icon ? $t('flow.header.changeIcon') : $t('flow.header.addIcon')}
				</button>
			{/if}
			{#if menu === 'icon' && !readOnly}
				<div class="absolute z-20 mt-1 w-72 rounded-md border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-900">
					<div class="grid grid-cols-8 gap-1">
						{#each PAGE_EMOJIS as emoji (emoji)}
							<button type="button" class="rounded p-1 text-xl hover:bg-slate-100 dark:hover:bg-slate-800" onclick={() => save(withAppearance({ icon: { kind: 'emoji', emoji } }))}>
								{emoji}
							</button>
						{/each}
					</div>
					<label class="mt-2 block text-xs text-slate-500">
						{$t('flow.header.uploadIcon')}
						<input class="mt-1 block w-full text-sm" type="file" accept="image/png,image/jpeg,image/webp,image/gif" onchange={(event) => onUpload(event, 'icon')} />
					</label>
					{#if appearance?.icon}
						<button type="button" class="mt-2 text-xs text-slate-500" onclick={() => save(withAppearance({ icon: null }))}>
							{$t('flow.header.removeIcon')}
						</button>
					{/if}
				</div>
			{/if}
		</div>

		<div class="flex items-start gap-3">
			<input
				value={title}
				oninput={(event) => {
					const target = event.currentTarget;
					if (target instanceof HTMLInputElement) onTitle?.(target.value);
				}}
				readonly={readOnly}
				placeholder={$t('flow.titlePlaceholder')}
				aria-label={$t('flow.panel.title')}
				class="min-w-0 flex-1 border-none bg-transparent text-4xl font-semibold text-slate-900 outline-none dark:text-slate-100"
			/>
			{#if !publicToken}
				<FlowSyncIndicator state={syncState} {pending} />
			{/if}
		</div>

		{#if !readOnly}
			<div class="relative mt-2">
				<button type="button" class="text-sm text-slate-500 hover:text-slate-800" onclick={() => (menu = menu === 'cover' ? null : 'cover')}>
					{cover ? $t('flow.header.changeCover') : $t('flow.header.addCover')}
				</button>
				{#if menu === 'cover'}
					<div class="absolute z-20 mt-1 w-80 rounded-md border border-slate-200 bg-white p-3 shadow-lg dark:border-slate-700 dark:bg-slate-900">
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
							<input class="mt-1 block w-full text-sm" type="file" accept="image/png,image/jpeg,image/webp,image/gif" onchange={(event) => onUpload(event, 'cover')} />
						</label>
						{#if cover}
							<button type="button" class="mt-2 text-xs text-slate-500" onclick={() => save(withAppearance({ cover: null }))}>
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
