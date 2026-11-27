<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import GripVertical from '@lucide/svelte/icons/grip-vertical';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Plus from '@lucide/svelte/icons/plus';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { describeResult } from '#lib/i18n/describe.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import { navApi } from '../api';
	import { builtinInfo } from '../builtins';
	import { navStore } from '../store.svelte';
	import type { NavDoc, NavEntryDoc, NavScope } from '../types';
	import EntryDialog from './EntryDialog.svelte';
	import {
		MAX_ENTRIES,
		MAX_SECTIONS,
		addEntry,
		addSection,
		cleanLabels,
		entryCount,
		findEntry,
		moveEntry,
		removeEntry,
		removeSection,
		sameDoc,
		stepEntry,
		stepSection,
		updateEntry,
		updateSection
	} from './logic';

	/**
	 * The administrator's editor of one navigation document (workspace or project): sections and
	 * entries in order, with drag-and-drop and buttons (Alt+Arrow on a row) for the keyboard.
	 * Saving sends `expected_version`; a newer version asks for a reload instead of overwriting.
	 */
	let { scope, scopeId, workspaceId }: { scope: NavScope; scopeId: string; workspaceId: string } =
		$props();

	type Load =
		| { status: 'loading' }
		| { status: 'ready' }
		| { status: 'forbidden' }
		| { status: 'failed' };

	let load = $state.raw<Load>({ status: 'loading' });
	let draft = $state.raw<NavDoc | null>(null);
	let saved = $state.raw<NavDoc | null>(null);
	let version = $state(0);
	let isDefault = $state(true);
	let broken = $state.raw<string[]>([]);
	/** Names of the objects entries point at, from the resolved menu and from the picker. */
	let names = $state.raw<Record<string, string>>({});
	let saving = $state(false);
	let problem = $state<string | null>(null);
	let conflict = $state(false);
	let announcement = $state('');
	let dialog = $state<{ entry: NavEntryDoc | null; section: string } | null>(null);

	const dirty = $derived(draft !== null && saved !== null && !sameDoc(draft, saved));

	function adopt(config: { doc: NavDoc; version: number; is_default: boolean; broken: string[] }) {
		draft = config.doc;
		saved = config.doc;
		version = config.version;
		isDefault = config.is_default;
		broken = config.broken;
	}

	async function reload() {
		load = { status: 'loading' };
		problem = null;
		conflict = false;
		try {
			const [config, menu] = await Promise.all([
				navApi.config(scope, scopeId),
				navApi.menu(scope, scopeId, { includeUserHidden: true })
			]);
			if (config.code === 0 && config.data) {
				adopt(config.data);
				const found: Record<string, string> = {};
				for (const section of menu.data?.sections ?? [])
					for (const e of section.entries) if (e.name) found[e.id] = e.name;
				names = found;
				load = { status: 'ready' };
			} else load = { status: config.code === 403 ? 'forbidden' : 'failed' };
		} catch {
			load = { status: 'failed' };
		}
	}

	$effect(() => {
		void scope;
		void scopeId;
		void reload();
	});

	function say(key: string, values?: Record<string, string>) {
		announcement = $t(key, values ? { values } : undefined);
	}

	// --- text of an entry ---
	function title(entry: NavEntryDoc): string {
		const own = entry.label?.en ?? entry.label?.zh;
		if (own) return own;
		if (entry.target.kind === 'builtin') {
			const info = builtinInfo(entry.target.page);
			return info ? $t(info.labelKey) : entry.target.page;
		}
		if (entry.target.kind === 'external') return entry.target.url;
		if (entry.target.kind === 'plugin_app')
			return `${entry.target.package_key} / ${entry.target.route}`;
		return names[entry.id] ?? entry.id;
	}

	// --- changes ---
	function change(next: NavDoc | null) {
		if (next) draft = next;
	}

	function step(entry: NavEntryDoc, direction: -1 | 1) {
		if (!draft) return;
		const next = stepEntry(draft, entry.id, direction);
		if (next === draft) return;
		draft = next;
		const at = findEntry(next, entry.id);
		if (at)
			say('nav.editor.moved', {
				name: title(entry),
				position: String(at.index + 1),
				total: String(next.sections[at.section].entries.length)
			});
		void focusRow(entry.id);
	}

	async function focusRow(id: string) {
		await Promise.resolve();
		requestAnimationFrame(() => {
			document.querySelector<HTMLElement>(`[data-nav-handle="${CSS.escape(id)}"]`)?.focus();
		});
	}

	function onRowKey(event: KeyboardEvent, entry: NavEntryDoc) {
		if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
		event.preventDefault();
		step(entry, event.key === 'ArrowUp' ? -1 : 1);
	}

	// --- drag and drop (native, as the task tab strip) ---
	let dragId = $state<string | null>(null);
	let dropAt = $state<{ section: number; index: number } | null>(null);

	function onDragOver(event: DragEvent, section: number, index: number | null) {
		if (!dragId || !draft) return;
		event.preventDefault();
		const row = (event.currentTarget as HTMLElement).closest<HTMLElement>('[data-nav-row]');
		let at = index ?? draft.sections[section].entries.length;
		if (row && index !== null) {
			const box = row.getBoundingClientRect();
			if (event.clientY > box.top + box.height / 2) at = index + 1;
		}
		dropAt = { section, index: at };
	}

	function onDrop(event: DragEvent) {
		event.preventDefault();
		if (draft && dragId && dropAt) {
			const from = findEntry(draft, dragId);
			let index = dropAt.index;
			// The target index counts after the entry is lifted out of the same section.
			if (from && from.section === dropAt.section && from.index < index) index -= 1;
			const entry = from?.entry;
			change(moveEntry(draft, dragId, { section: dropAt.section, index }));
			if (entry) say('nav.editor.droppedAt', { name: title(entry) });
		}
		dragId = null;
		dropAt = null;
	}

	// --- sections ---
	function newSection() {
		if (!draft) return;
		change(addSection(draft, { en: $t('nav.editor.newSection') }));
	}

	function renameSection(id: string, locale: 'en' | 'zh', text: string) {
		if (!draft) return;
		const current = draft.sections.find((s) => s.id === id)?.label ?? {};
		change(updateSection(draft, id, { label: cleanLabels({ ...current, [locale]: text }) }));
	}

	// --- entries ---
	function openAdd(sectionId: string) {
		dialog = { entry: null, section: sectionId };
	}

	function applyEntry(entry: NavEntryDoc, name: string | null) {
		if (!draft || !dialog) return;
		const next = dialog.entry
			? updateEntry(draft, dialog.entry.id, entry)
			: addEntry(draft, dialog.section, entry);
		if (!next) {
			problem = $t('nav.editor.limit', { values: { entries: MAX_ENTRIES } });
			return;
		}
		change(next);
		if (name) names = { ...names, [entry.id]: name };
		dialog = null;
	}

	async function removeOne(entry: NavEntryDoc) {
		if (!draft) return;
		const ok = await confirmAction({
			title: $t('nav.editor.removeTitle', { values: { name: title(entry) } }),
			body: $t('nav.editor.removeBody'),
			verb: 'remove',
			object: title(entry)
		});
		if (ok && draft) change(removeEntry(draft, entry.id));
	}

	// --- save and reset ---
	function failure(result: Awaited<ReturnType<typeof navApi.saveConfig>>): string {
		const details = result.details as { path?: string; reason?: string } | undefined;
		if (details?.reason) {
			const key = `nav.editor.errors.${details.reason}`;
			const text = $t(key);
			if (text !== key) return text;
		}
		return describeResult(result, $t('nav.editor.saveFailed'));
	}

	async function save() {
		if (!draft || saving) return;
		saving = true;
		problem = null;
		conflict = false;
		try {
			const result = await navApi.saveConfig(scope, scopeId, draft, version);
			if (result.code === 0 && result.data) {
				adopt(result.data);
				void navStore.invalidate();
				say('nav.editor.saved');
			} else if (result.error_code === 'version_conflict') conflict = true;
			else problem = failure(result);
		} finally {
			saving = false;
		}
	}

	async function reset() {
		const ok = await confirmAction({
			title: $t('nav.editor.resetTitle'),
			body: $t('nav.editor.resetBody'),
			confirmLabel: $t('nav.editor.reset')
		});
		if (!ok) return;
		const result = await navApi.resetConfig(scope, scopeId);
		if (result.code === 0 && result.data) {
			adopt(result.data);
			names = {};
			void navStore.invalidate();
			say('nav.editor.resetDone');
		} else problem = describeResult(result, $t('nav.editor.saveFailed'));
	}
</script>

<div class="editor" data-testid="nav-editor" data-nav-scope={scope}>
	<header class="head">
		<h1>
			{scope === 'workspace' ? $t('nav.editor.workspaceTitle') : $t('nav.editor.projectTitle')}
		</h1>
		<p class="lead">{$t('nav.editor.lead')}</p>
	</header>

	{#if load.status === 'loading'}
		<StateBlock kind="loading" thing={$t('nav.editor.thing')} />
	{:else if load.status === 'forbidden'}
		<StateBlock kind="forbidden" thing={$t('nav.editor.thing')} />
	{:else if load.status === 'failed' || !draft}
		<StateBlock
			kind="loadFailed"
			thing={$t('nav.editor.thing')}
			action={{ run: () => void reload() }}
		/>
	{:else}
		{#if conflict}
			<SecurityWarning title={$t('nav.editor.conflictTitle')}>
				{$t('nav.editor.conflictBody')}
				<Button size="sm" onclick={() => void reload()}>{$t('nav.editor.reload')}</Button>
			</SecurityWarning>
		{/if}
		{#if problem}
			<SecurityWarning tone="danger" title={$t('nav.editor.saveFailed')}>{problem}</SecurityWarning>
		{/if}

		<div class="bar">
			<Button
				variant="primary"
				loading={saving}
				loadingLabel={$t('nav.editor.saving')}
				disabled={!dirty}
				disabledReason={$t('nav.editor.nothingToSave')}
				onclick={() => void save()}>{$t('nav.editor.save')}</Button
			>
			<Button variant="text" disabled={!dirty} onclick={() => (draft = saved)}>
				{$t('nav.editor.discard')}
			</Button>
			<Button variant="danger-quiet" disabled={isDefault} onclick={() => void reset()}>
				{$t('nav.editor.reset')}
			</Button>
			{#if dirty}<span class="unsaved" data-nav-unsaved>{$t('nav.editor.unsaved')}</span>{/if}
		</div>

		{#each draft.sections as section, s (section.id)}
			<section
				class="section"
				data-nav-section={section.id}
				aria-label={section.label?.en ?? section.id}
			>
				<div class="section-head">
					<TextField
						label={$t('nav.editor.sectionEn')}
						value={section.label?.en ?? ''}
						maxlength={64}
						oninput={(e) => renameSection(section.id, 'en', e.currentTarget.value)}
					/>
					<TextField
						label={$t('nav.editor.sectionZh')}
						value={section.label?.zh ?? ''}
						maxlength={64}
						oninput={(e) => renameSection(section.id, 'zh', e.currentTarget.value)}
					/>
					<Switch
						checked={section.collapsed}
						label={$t('nav.editor.collapsed')}
						onchange={(e) =>
							draft &&
							change(updateSection(draft, section.id, { collapsed: e.currentTarget.checked }))}
					/>
					<div class="tools">
						<IconButton
							icon={ArrowUp}
							label={$t('nav.editor.sectionUp')}
							disabled={s === 0}
							onclick={() => draft && change(stepSection(draft, section.id, -1))}
						/>
						<IconButton
							icon={ArrowDown}
							label={$t('nav.editor.sectionDown')}
							disabled={s === draft.sections.length - 1}
							onclick={() => draft && change(stepSection(draft, section.id, 1))}
						/>
						<IconButton
							icon={Trash2}
							label={$t('nav.editor.sectionRemove')}
							disabled={section.entries.length > 0}
							onclick={() => draft && change(removeSection(draft, section.id))}
						/>
					</div>
				</div>
				<ul
					class="entries"
					class:drop-empty={dropAt?.section === s && section.entries.length === 0}
					ondragover={(e) => onDragOver(e, s, null)}
					ondrop={onDrop}
				>
					{#each section.entries as entry, i (entry.id)}
						<li
							class="entry"
							class:dragging={dragId === entry.id}
							class:drop-before={dropAt?.section === s && dropAt.index === i}
							class:drop-after={dropAt?.section === s &&
								dropAt.index === section.entries.length &&
								i === section.entries.length - 1}
							data-nav-row={entry.id}
							draggable="true"
							ondragstart={(e) => {
								dragId = entry.id;
								e.dataTransfer?.setData('text/plain', entry.id);
								if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
							}}
							ondragover={(e) => {
								e.stopPropagation();
								onDragOver(e, s, i);
							}}
							ondrop={onDrop}
							ondragend={() => {
								dragId = null;
								dropAt = null;
							}}
						>
							<button
								type="button"
								class="handle"
								data-nav-handle={entry.id}
								aria-label={$t('nav.editor.handle', { values: { name: title(entry) } })}
								aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
								onkeydown={(e) => onRowKey(e, entry)}
							>
								<GripVertical size={16} aria-hidden="true" />
							</button>
							<div class="entry-text">
								<span class="entry-title">{title(entry)}</span>
								<span class="entry-kind">{$t(`nav.editor.kinds.${entry.target.kind}`)}</span>
							</div>
							<div class="chips">
								{#if broken.includes(entry.id)}
									<span class="chip chip-warn" data-nav-broken>{$t('nav.editor.broken')}</span>
								{/if}
								{#if entry.hidden}<span class="chip">{$t('nav.editor.chipHidden')}</span>{/if}
								{#if entry.pinned}<span class="chip">{$t('nav.editor.chipPinned')}</span>{/if}
								{#if entry.locked}<span class="chip">{$t('nav.editor.chipLocked')}</span>{/if}
							</div>
							<div class="tools">
								<IconButton
									icon={ArrowUp}
									label={$t('nav.editor.up', { values: { name: title(entry) } })}
									disabled={s === 0 && i === 0}
									onclick={() => step(entry, -1)}
								/>
								<IconButton
									icon={ArrowDown}
									label={$t('nav.editor.down', { values: { name: title(entry) } })}
									disabled={s === draft.sections.length - 1 && i === section.entries.length - 1}
									onclick={() => step(entry, 1)}
								/>
								<IconButton
									icon={Pencil}
									label={$t('nav.editor.edit', { values: { name: title(entry) } })}
									onclick={() => (dialog = { entry, section: section.id })}
								/>
								<IconButton
									icon={Trash2}
									label={$t('nav.editor.remove', { values: { name: title(entry) } })}
									onclick={() => void removeOne(entry)}
								/>
							</div>
						</li>
					{:else}
						<li class="empty">{$t('nav.editor.emptySection')}</li>
					{/each}
				</ul>
				<div>
					<Button
						size="sm"
						disabled={entryCount(draft) >= MAX_ENTRIES}
						onclick={() => openAdd(section.id)}
					>
						<Plus size={14} aria-hidden="true" />
						{$t('nav.editor.addEntry')}
					</Button>
				</div>
			</section>
		{/each}

		<div>
			<Button disabled={draft.sections.length >= MAX_SECTIONS} onclick={newSection}>
				<Plus size={14} aria-hidden="true" />
				{$t('nav.editor.addSection')}
			</Button>
		</div>

		{#if dialog}
			<EntryDialog
				open
				{scope}
				{scopeId}
				{workspaceId}
				doc={draft}
				entry={dialog.entry}
				onsave={applyEntry}
				onclose={() => (dialog = null)}
			/>
		{/if}
	{/if}
	<div class="sr-only" role="status" aria-live="polite" data-nav-live>{announcement}</div>
</div>

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-8);
		padding: var(--rf-space-9);
		max-width: 56rem;
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	h1 {
		margin: 0;
		font-size: var(--rf-size-page);
		line-height: var(--rf-leading-title);
		font-weight: var(--rf-weight-semibold);
	}
	.lead {
		margin: var(--rf-space-2) 0 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.unsaved {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.section {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-6);
		border: var(--rf-border-width) solid var(--rf-border);
		border-radius: var(--rf-radius-md);
	}
	.section-head {
		display: flex;
		flex-wrap: wrap;
		align-items: end;
		gap: var(--rf-space-6);
	}
	.entries {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-height: var(--rf-space-9);
	}
	.entries.drop-empty {
		outline: var(--rf-border-width-focus) dashed var(--rf-primary);
	}
	.entry {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		padding: var(--rf-space-3) var(--rf-space-4);
		border: var(--rf-border-width) solid var(--rf-border);
		border-radius: var(--rf-radius-sm);
		background: var(--rf-surface);
	}
	.entry.dragging {
		opacity: 0.5;
	}
	.entry.drop-before {
		box-shadow: 0 calc(-1 * var(--rf-border-width-focus)) 0 0 var(--rf-primary);
	}
	.entry.drop-after {
		box-shadow: 0 var(--rf-border-width-focus) 0 0 var(--rf-primary);
	}
	.handle {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: var(--rf-space-8);
		height: var(--rf-space-8);
		border: 0;
		border-radius: var(--rf-radius-xs);
		background: none;
		color: var(--rf-secondary);
		cursor: grab;
	}
	.handle:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
	}
	.entry-text {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}
	.entry-title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.entry-kind {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.chips,
	.tools {
		display: flex;
		align-items: center;
		gap: var(--rf-space-2);
	}
	.chip {
		padding: 0 var(--rf-space-3);
		border-radius: var(--rf-radius-xs);
		background: var(--rf-hover);
		font-size: var(--rf-size-meta);
	}
	.chip-warn {
		color: var(--rf-danger);
	}
	.empty {
		padding: var(--rf-space-4);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
</style>
