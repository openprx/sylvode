<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { PluginContributionItem, PluginSlotV3 } from '#lib/api/plugin-packages.js';
	import {
		clearAllPreferences,
		clearPreference,
		effectivePosition,
		itemKey,
		moveDownPosition,
		moveUpPosition,
		preferenceError,
		savePreference,
		sortItems
	} from '#lib/plugins/contributions.js';

	let {
		projectId,
		items
	}: {
		projectId: string;
		/** Every protocol 3 item of the surface, including personally hidden ones. */
		items: PluginContributionItem[];
	} = $props();

	const slotLabels: Record<PluginSlotV3, string> = {
		'form.record.detail': 'plugins.prefs.slots.recordDetail',
		'form.record.actions': 'plugins.prefs.slots.recordActions',
		'form.list.toolbar': 'plugins.prefs.slots.listToolbar',
		'project.view': 'plugins.prefs.slots.projectView'
	};
	const slots = $derived(
		(Object.keys(slotLabels) as PluginSlotV3[])
			.map((slot) => ({
				slot,
				entries: sortItems(items.filter((item) => item.contribution.slot === slot))
			}))
			.filter((group) => group.entries.length > 0)
	);
	const customized = $derived(
		items.some(
			(item) =>
				item.preference.hidden || item.preference.collapsed || item.preference.position !== null
		)
	);
	let open = $state(false);
	let busy = $state(false);
	let message = $state('');

	async function run(action: () => Promise<{ code: number; details?: unknown }>, done: string) {
		if (busy) return;
		busy = true;
		const response = await action();
		busy = false;
		message = response.code === 0 ? done : preferenceError(response);
	}

	const update = (
		item: PluginContributionItem,
		change: Partial<PluginContributionItem['preference']>
	) =>
		run(
			() => savePreference(projectId, item, { ...item.preference, ...change }),
			'plugins.prefs.saved'
		);

	function setPosition(item: PluginContributionItem, raw: string) {
		const value = Number(raw);
		if (raw.trim() === '' || !Number.isInteger(value) || value < 0 || value > 99) {
			message = 'plugins.prefs.saveFailed';
			return;
		}
		void update(item, { position: value });
	}
</script>

{#if slots.length}
	<div class="relative inline-block" data-plugin-prefs>
		<button
			type="button"
			class="rounded-control px-2 py-1 text-sm text-fg underline"
			aria-expanded={open}
			onclick={() => (open = !open)}>{$t('plugins.prefs.menu')}</button
		>
		{#if open}
			<div
				role="group"
				aria-label={$t('plugins.prefs.menu')}
				class="absolute right-0 z-30 mt-1 w-80 max-w-[90vw] space-y-3 rounded-float border border-overlay-line bg-overlay p-3 text-sm text-fg shadow-overlay"
			>
				{#each slots as group (group.slot)}
					<section>
						<h4 class="text-xs font-semibold uppercase text-fg-muted">
							{$t(slotLabels[group.slot])}
						</h4>
						<ul class="mt-1 space-y-2">
							{#each group.entries as item (itemKey(item))}
								{@const up = moveUpPosition(group.entries, item)}
								{@const down = moveDownPosition(group.entries, item)}
								<li
									class="space-y-1 rounded-card border border-line p-2"
									data-plugin-pref={itemKey(item)}
								>
									<p class="font-medium">
										{item.contribution.label}
										{#if item.preference.hidden}
											<span class="text-xs font-normal text-fg-muted"
												>· {$t('plugins.prefs.hiddenSection')}</span
											>
										{/if}
									</p>
									<div class="flex flex-wrap items-center gap-2">
										<button
											type="button"
											class="underline"
											disabled={busy}
											onclick={() => update(item, { hidden: !item.preference.hidden })}
											>{item.preference.hidden
												? $t('plugins.prefs.restore')
												: $t('plugins.prefs.hide')}</button
										>
										{#if !item.preference.hidden}
											<button
												type="button"
												class="underline"
												disabled={busy}
												onclick={() => update(item, { collapsed: !item.preference.collapsed })}
												>{item.preference.collapsed
													? $t('plugins.prefs.expand')
													: $t('plugins.prefs.collapse')}</button
											>
										{/if}
										<button
											type="button"
											class="underline disabled:text-control"
											disabled={busy || up === null}
											onclick={() => up !== null && update(item, { position: up })}
											>{$t('plugins.prefs.moveUp')}</button
										>
										<button
											type="button"
											class="underline disabled:text-control"
											disabled={busy || down === null}
											onclick={() => down !== null && update(item, { position: down })}
											>{$t('plugins.prefs.moveDown')}</button
										>
										<button
											type="button"
											class="underline"
											disabled={busy}
											onclick={() =>
												run(() => clearPreference(projectId, item), 'plugins.prefs.resetDone')}
											>{$t('plugins.prefs.reset')}</button
										>
									</div>
									<label class="flex items-center gap-2 text-xs">
										<span>{$t('plugins.prefs.positionLabel')}</span>
										<input
											type="number"
											min="0"
											max="99"
											step="1"
											value={effectivePosition(item)}
											disabled={busy}
											class="w-16 rounded-control border border-input bg-field px-1 py-0.5"
											onchange={(event) => setPosition(item, event.currentTarget.value)}
										/>
									</label>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
				{#if customized}
					<button
						type="button"
						class="underline"
						disabled={busy}
						onclick={() => run(() => clearAllPreferences(projectId), 'plugins.prefs.resetDone')}
						>{$t('plugins.prefs.resetAll')}</button
					>
				{/if}
				{#if message}
					<p role="status" class="text-xs text-fg-muted">{$t(message)}</p>
				{/if}
			</div>
		{/if}
	</div>
{/if}
