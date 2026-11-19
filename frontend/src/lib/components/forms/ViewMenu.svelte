<script lang="ts">
	/**
	 * Everything about the current view in one menu (APG menu button): update it with the
	 * working changes, rename, share or make private, set as default, layout settings, duplicate,
	 * reset and delete (with an inline confirmation). Items the user may not use are not shown.
	 */
	import { tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		Copy,
		Lock,
		MoreHorizontal,
		Pencil,
		RotateCcw,
		Save,
		Settings2,
		Star,
		StarOff,
		Trash2,
		Users
	} from '@lucide/svelte';
	import type { FormView } from '#lib/api/forms.js';
	import Popover from './Popover.svelte';

	interface Props {
		/** `null` is the built-in "All records" table. */
		view: FormView | null;
		modified: boolean;
		can: { manage: boolean; share: boolean; setDefault: boolean; create: boolean };
		isDefault: boolean;
		isPrivate: boolean;
		busy?: boolean;
		onupdate: () => void;
		onrename: (name: string) => void;
		onvisibility: (visibility: 'shared' | 'private') => void;
		ondefault: (on: boolean) => void;
		onlayout: () => void;
		onduplicate: () => void;
		onreset: () => void;
		ondelete: () => void;
	}

	let {
		view,
		modified,
		can,
		isDefault,
		isPrivate,
		busy = false,
		onupdate,
		onrename,
		onvisibility,
		ondefault,
		onlayout,
		onduplicate,
		onreset,
		ondelete
	}: Props = $props();

	let open = $state(false);
	let stage = $state<'menu' | 'rename' | 'confirm-delete'>('menu');
	let button = $state<HTMLButtonElement | null>(null);
	let panel = $state<HTMLElement | null>(null);
	let renameValue = $state('');

	function items(): HTMLElement[] {
		return Array.from(panel?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
	}

	async function show() {
		open = true;
		stage = 'menu';
		await tick();
		await tick();
		items()[0]?.focus();
	}

	function close(returnFocus = true) {
		open = false;
		stage = 'menu';
		if (returnFocus) button?.focus();
	}

	function run(action: () => void) {
		close();
		action();
	}

	async function startRename() {
		renameValue = view?.name ?? '';
		stage = 'rename';
		await tick();
		const input = panel?.querySelector<HTMLInputElement>('[data-view-rename-input]');
		input?.focus();
		input?.select();
	}

	function submitRename() {
		const name = renameValue.trim();
		if (!name || name.length > 80) return;
		run(() => onrename(name));
	}

	async function confirmDelete() {
		stage = 'confirm-delete';
		await tick();
		panel?.querySelector<HTMLElement>('[data-view-delete-cancel]')?.focus();
	}

	function onMenuKey(event: KeyboardEvent) {
		if (stage !== 'menu') return;
		const list = items();
		const index = list.indexOf(document.activeElement as HTMLElement);
		if (event.key === 'ArrowDown') list[(index + 1) % list.length]?.focus();
		else if (event.key === 'ArrowUp') list[(index - 1 + list.length) % list.length]?.focus();
		else if (event.key === 'Home') list[0]?.focus();
		else if (event.key === 'End') list[list.length - 1]?.focus();
		else return;
		event.preventDefault();
	}

	const itemClass =
		'flex min-h-8 w-full items-center gap-2 rounded-md px-2 py-1.5 text-start text-sm text-slate-700 hover:bg-slate-100 focus:bg-slate-100 focus:outline-none dark:text-slate-200 dark:hover:bg-slate-800 dark:focus:bg-slate-800';
</script>

<button
	type="button"
	bind:this={button}
	class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-600 hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-500 dark:text-slate-300 dark:hover:bg-slate-800"
	aria-haspopup="menu"
	aria-expanded={open}
	aria-label={$t('forms.views.viewMenu')}
	title={$t('forms.views.viewMenu')}
	data-view-menu-button
	onclick={() => (open ? close() : void show())}
	onkeydown={(event) => {
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			void show();
		}
	}}><MoreHorizontal class="h-4 w-4" aria-hidden="true" /></button
>

<Popover
	{open}
	anchor={button}
	onclose={() => close(false)}
	role={stage === 'menu' ? 'menu' : 'dialog'}
	label={$t('forms.views.viewMenu')}
	width={280}
	autofocus={false}
	testId="view-menu"
>
	<div bind:this={panel} role="none" onkeydown={onMenuKey}>
		{#if stage === 'rename'}
			<form
				class="space-y-2 p-1"
				onsubmit={(event) => {
					event.preventDefault();
					submitRename();
				}}
			>
				<label class="block space-y-1">
					<span class="text-xs font-medium text-slate-600 dark:text-slate-300"
						>{$t('forms.views.renameTitle')}</span
					>
					<input
						class="h-8 w-full rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-800"
						maxlength="80"
						bind:value={renameValue}
						data-view-rename-input
					/>
				</label>
				<div class="flex justify-end gap-2">
					<button
						type="button"
						class="h-8 rounded-md px-3 text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
						onclick={() => (stage = 'menu')}>{$t('common.cancel')}</button
					>
					<button
						type="submit"
						class="h-8 rounded-md bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
						disabled={!renameValue.trim()}
						data-view-rename-save>{$t('common.save')}</button
					>
				</div>
			</form>
		{:else if stage === 'confirm-delete' && view}
			<div class="space-y-3 p-1" role="alertdialog" aria-labelledby="view-delete-question">
				<p id="view-delete-question" class="text-sm text-slate-800 dark:text-slate-100">
					{isPrivate
						? $t('forms.views.deletePrivateConfirm', { values: { name: view.name } })
						: $t('forms.views.deleteConfirm', { values: { name: view.name } })}
				</p>
				<div class="flex justify-end gap-2">
					<button
						type="button"
						class="h-8 rounded-md px-3 text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
						data-view-delete-cancel
						onclick={() => (stage = 'menu')}>{$t('common.cancel')}</button
					>
					<button
						type="button"
						class="h-8 rounded-md bg-red-600 px-3 text-sm font-medium text-white hover:bg-red-700"
						data-view-delete-confirm
						onclick={() => run(ondelete)}>{$t('common.delete')}</button
					>
				</div>
			</div>
		{:else}
			{#if view && modified && can.manage}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					disabled={busy}
					data-view-menu-update
					onclick={() => run(onupdate)}
					><Save class="h-4 w-4 shrink-0" aria-hidden="true" /><span class="min-w-0 truncate"
						>{$t('forms.views.updateWithChanges', { values: { name: view.name } })}</span
					></button
				>
			{/if}
			{#if view && can.manage}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					data-view-menu-rename
					onclick={startRename}
					><Pencil class="h-4 w-4" aria-hidden="true" />{$t('forms.views.rename')}</button
				>
			{/if}
			{#if view && can.manage && can.share}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					data-view-menu-visibility={isPrivate ? 'shared' : 'private'}
					onclick={() => run(() => onvisibility(isPrivate ? 'shared' : 'private'))}
				>
					{#if isPrivate}
						<Users class="h-4 w-4" aria-hidden="true" />{$t('forms.views.share')}
					{:else}
						<Lock class="h-4 w-4" aria-hidden="true" />{$t('forms.views.makePrivate')}
					{/if}
				</button>
			{/if}
			{#if view && can.setDefault && (!isPrivate || isDefault)}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					data-view-menu-default={isDefault ? 'off' : 'on'}
					onclick={() => run(() => ondefault(!isDefault))}
				>
					{#if isDefault}
						<StarOff class="h-4 w-4" aria-hidden="true" />{$t('forms.views.removeDefault')}
					{:else}
						<Star class="h-4 w-4" aria-hidden="true" />{$t('forms.views.setDefault')}
					{/if}
				</button>
			{/if}
			<button
				type="button"
				role="menuitem"
				tabindex="-1"
				class={itemClass}
				data-view-menu-layout
				onclick={() => run(onlayout)}
				><Settings2 class="h-4 w-4" aria-hidden="true" />{$t('forms.views.layoutSettings')}</button
			>
			{#if view && can.create}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					data-view-menu-duplicate
					onclick={() => run(onduplicate)}
					><Copy class="h-4 w-4" aria-hidden="true" />{$t('forms.views.duplicate')}</button
				>
			{/if}
			{#if modified}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={itemClass}
					data-view-menu-reset
					onclick={() => run(onreset)}
					><RotateCcw class="h-4 w-4" aria-hidden="true" />{$t('forms.views.reset')}</button
				>
			{/if}
			{#if view && can.manage}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class="{itemClass} text-red-700 dark:text-red-400"
					data-view-menu-delete
					onclick={confirmDelete}
					><Trash2 class="h-4 w-4" aria-hidden="true" />{$t('forms.views.deleteView')}</button
				>
			{/if}
		{/if}
	</div>
</Popover>
