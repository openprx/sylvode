<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import Eye from '@lucide/svelte/icons/eye';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import GripVertical from '@lucide/svelte/icons/grip-vertical';
	import Lock from '@lucide/svelte/icons/lock';
	import Star from '@lucide/svelte/icons/star';
	import { describeResult } from '#lib/i18n/describe.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { navApi } from './api';
	import { builtinInfo } from './builtins';
	import {
		MAX_PINNED,
		buildSections,
		emptyPrefs,
		isEmpty,
		moveWithin,
		normalisePrefs,
		pinCount,
		setHidden,
		setPinned,
		setSectionCollapsed,
		stepWithin,
		type SheetEntry
	} from './customise';
	import { pickLabel, sectionCaption } from './rows';
	import { navStore } from './store.svelte';
	import type { NavPrefs, NavScope, ResolvedNav } from './types';

	/**
	 * The signed-in person's own view of one navigation: hide, pin and reorder entries, and keep a
	 * section open or closed. Locked entries show a lock and do not respond. Changes are kept in
	 * preferences (`PUT /me/nav-prefs`), saved half a second after the last one.
	 */
	let {
		open,
		scope,
		scopeId,
		workspaceId,
		projectId = null,
		onclose
	}: {
		open: boolean;
		scope: NavScope;
		scopeId: string;
		workspaceId: string;
		projectId?: string | null;
		onclose: () => void;
	} = $props();

	type Load = { status: 'loading' } | { status: 'ready' } | { status: 'failed' };
	let load = $state.raw<Load>({ status: 'loading' });
	let menu = $state.raw<ResolvedNav | null>(null);
	let initial = $state.raw<NavPrefs>(emptyPrefs());
	let prefs = $state.raw<NavPrefs>(emptyPrefs());
	let problem = $state<string | null>(null);
	let announcement = $state('');
	let dragId = $state<string | null>(null);
	let dropAt = $state<{ section: string; index: number } | null>(null);

	async function reload() {
		load = { status: 'loading' };
		problem = null;
		try {
			const [m, p] = await Promise.all([
				navApi.menu(scope, scopeId, { includeUserHidden: true }),
				navApi.prefs(workspaceId, projectId)
			]);
			if (m.code === 0 && m.data && p.code === 0 && p.data) {
				menu = m.data;
				initial = normalisePrefs(p.data.prefs);
				prefs = initial;
				load = { status: 'ready' };
			} else load = { status: 'failed' };
		} catch {
			load = { status: 'failed' };
		}
	}

	$effect(() => {
		if (open) void reload();
	});

	const sections = $derived(menu ? buildSections(menu, initial, prefs) : []);
	const pins = $derived(pinCount(sections));

	// --- saving (debounced, one request at a time) ---
	let timer: ReturnType<typeof setTimeout> | null = null;
	let pending = false;
	let inFlight: Promise<void> | null = null;

	function change(next: NavPrefs) {
		if (next === prefs) return;
		prefs = next;
		pending = true;
		if (timer) clearTimeout(timer);
		timer = setTimeout(() => void flush(), 500);
	}

	async function flush(): Promise<void> {
		if (timer) clearTimeout(timer);
		timer = null;
		if (inFlight) await inFlight;
		if (!pending) return;
		pending = false;
		const body = prefs;
		inFlight = (async () => {
			const result = isEmpty(body)
				? await navApi.resetPrefs(workspaceId, projectId)
				: await navApi.savePrefs(workspaceId, projectId, body);
			if (result.code === 0) {
				problem = null;
				void navStore.invalidate();
			} else problem = describeResult(result, $t('nav.customise.saveFailed'));
		})();
		await inFlight;
		inFlight = null;
	}

	async function close() {
		await flush();
		onclose();
	}

	async function resetAll() {
		if (timer) clearTimeout(timer);
		timer = null;
		pending = false;
		const result = await navApi.resetPrefs(workspaceId, projectId);
		if (result.code === 0) {
			void navStore.invalidate();
			announcement = $t('nav.customise.resetDone');
			await reload();
		} else problem = describeResult(result, $t('nav.customise.saveFailed'));
	}

	// --- text ---
	function text(item: SheetEntry): string {
		const e = item.entry;
		const own = pickLabel(e.label, $locale ?? 'en');
		if (own) return own;
		if (e.target.kind === 'builtin') {
			const info = builtinInfo(e.target.page);
			return info ? $t(info.labelKey) : e.target.page;
		}
		return e.name ?? e.href ?? e.id;
	}

	// --- changes ---
	function step(sectionId: string, item: SheetEntry, direction: -1 | 1) {
		const section = sections.find((s) => s.id === sectionId);
		if (!section || item.entry.locked) return;
		const next = stepWithin(prefs, section, item.entry.id, direction);
		change(next);
		if (next !== prefs) announcement = $t('nav.editor.droppedAt', { values: { name: text(item) } });
		requestAnimationFrame(() =>
			document
				.querySelector<HTMLElement>(`[data-customise-handle="${CSS.escape(item.entry.id)}"]`)
				?.focus()
		);
	}

	function onDragOver(event: DragEvent, sectionId: string, index: number) {
		if (!dragId) return;
		event.preventDefault();
		const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
		dropAt = {
			section: sectionId,
			index: event.clientY > box.top + box.height / 2 ? index + 1 : index
		};
	}

	function onDrop(event: DragEvent) {
		event.preventDefault();
		const id = dragId;
		const at = dropAt;
		dragId = null;
		dropAt = null;
		if (!id || !at) return;
		const section = sections.find((s) => s.id === at.section);
		if (!section || !section.entries.some((e) => e.entry.id === id)) return;
		// Count only the unlocked entries before the drop point, after `id` is lifted out.
		const before = section.entries
			.slice(0, at.index)
			.filter((e) => !e.entry.locked && e.entry.id !== id);
		change(moveWithin(prefs, section, id, before.length));
	}
</script>

<Dialog
	{open}
	title={$t('nav.customise.title')}
	onclose={() => void close()}
	size="wide"
	testId="nav-customise"
>
	<div class="sheet">
		<p class="lead">{$t('nav.customise.lead')}</p>
		{#if load.status === 'loading'}
			<StateBlock kind="loading" thing={$t('nav.editor.thing')} />
		{:else if load.status === 'failed'}
			<StateBlock
				kind="loadFailed"
				thing={$t('nav.editor.thing')}
				action={{ run: () => void reload() }}
			/>
		{:else}
			{#if problem}
				<SecurityWarning tone="danger" title={$t('nav.customise.saveFailed')}
					>{problem}</SecurityWarning
				>
			{/if}
			{#each sections as section (section.id)}
				{@const caption = sectionCaption(section, $locale ?? 'en', (k) => $t(k))}
				<section class="group" data-customise-section={section.id}>
					{#if caption}
						<button
							type="button"
							class="caption"
							aria-expanded={!section.collapsed}
							onclick={() => change(setSectionCollapsed(prefs, section.id, !section.collapsed))}
						>
							<span class="chevron" class:open={!section.collapsed}>
								<ChevronRight size={14} aria-hidden="true" />
							</span>
							{caption}
						</button>
					{/if}
					<ul class="rows" ondragover={(e) => e.preventDefault()} ondrop={onDrop}>
						{#each section.entries as item, i (item.entry.id)}
							{@const locked = item.entry.locked}
							<li
								class="row"
								class:is-hidden={item.hidden}
								class:drop-before={dropAt?.section === section.id && dropAt.index === i}
								data-customise-row={item.entry.id}
								draggable={!locked}
								ondragstart={(e) => {
									dragId = item.entry.id;
									e.dataTransfer?.setData('text/plain', item.entry.id);
								}}
								ondragover={(e) => {
									e.stopPropagation();
									onDragOver(e, section.id, i);
								}}
								ondrop={onDrop}
								ondragend={() => {
									dragId = null;
									dropAt = null;
								}}
							>
								{#if locked}
									<span class="lock" title={$t('nav.customise.locked')} data-customise-locked>
										<Lock size={14} aria-hidden="true" />
										<span class="sr-only">{$t('nav.customise.locked')}</span>
									</span>
								{:else}
									<GripVertical size={16} aria-hidden="true" />
								{/if}
								<span class="name">{text(item)}</span>
								<div class="tools">
									<IconButton
										icon={ArrowUp}
										label={$t('nav.customise.up')}
										data-customise-handle={item.entry.id}
										disabled={locked}
										onclick={() => step(section.id, item, -1)}
									/>
									<IconButton
										icon={ArrowDown}
										label={$t('nav.customise.down')}
										disabled={locked}
										onclick={() => step(section.id, item, 1)}
									/>
									<IconButton
										icon={Star}
										label={item.pinned ? $t('nav.customise.unpin') : $t('nav.customise.pin')}
										pressed={item.pinned}
										disabled={locked || (!item.pinned && pins >= MAX_PINNED)}
										onclick={() => change(setPinned(prefs, item.entry, !item.pinned))}
									/>
									<IconButton
										icon={item.hidden ? Eye : EyeOff}
										label={item.hidden ? $t('nav.customise.show') : $t('nav.customise.hide')}
										disabled={locked}
										onclick={() => change(setHidden(prefs, item.entry, !item.hidden))}
									/>
								</div>
							</li>
						{/each}
					</ul>
				</section>
			{:else}
				<p class="lead">{$t('nav.customise.empty')}</p>
			{/each}
		{/if}
	</div>
	{#snippet actions()}
		<Button
			variant="danger-quiet"
			disabled={isEmpty(prefs) && isEmpty(initial)}
			onclick={() => void resetAll()}
		>
			{$t('nav.customise.reset')}
		</Button>
		<Button variant="primary" onclick={() => void close()}>{$t('nav.customise.close')}</Button>
	{/snippet}
	<div class="sr-only" role="status" aria-live="polite">{announcement}</div>
</Dialog>

<style>
	.sheet {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.lead {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.caption {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		align-self: flex-start;
		border: 0;
		background: none;
		padding: var(--rf-space-2) 0;
		color: var(--rf-secondary);
		font: inherit;
		font-size: var(--rf-size-meta);
		font-weight: var(--rf-weight-medium);
		cursor: pointer;
	}
	.caption:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
	}
	.chevron :global(svg) {
		transition: transform var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.chevron.open :global(svg) {
		transform: rotate(90deg);
	}
	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		padding: var(--rf-space-2) var(--rf-space-4);
		border: var(--rf-border-width) solid var(--rf-border);
		border-radius: var(--rf-radius-sm);
	}
	.row.is-hidden .name {
		color: var(--rf-secondary);
		text-decoration: line-through;
	}
	.row.drop-before {
		box-shadow: 0 calc(-1 * var(--rf-border-width-focus)) 0 0 var(--rf-primary);
	}
	.lock {
		display: inline-flex;
		color: var(--rf-secondary);
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tools {
		display: flex;
		gap: var(--rf-space-2);
	}
	@media (prefers-reduced-motion: reduce) {
		.chevron :global(svg) {
			transition: none;
		}
	}
</style>
