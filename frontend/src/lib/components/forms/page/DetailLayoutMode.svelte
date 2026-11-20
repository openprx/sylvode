<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ArrowDown, ArrowUp } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import { detailLayoutModes } from '#lib/forms/page-constants.js';
	import {
		detailPageFieldColumns,
		detailPageWidgetRegion
	} from '#lib/forms/detail-layout-drafts.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import DetailSectionsBuilder from '#lib/components/forms/page/DetailSectionsBuilder.svelte';
	import DetailWidgetsBuilder from '#lib/components/forms/page/DetailWidgetsBuilder.svelte';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { setDetailHeaderDraft, setDetailHighlightField, moveDetailHighlightField } =
		fp.detailLayoutEditor;
	const { saveDetailHeader, saveDetailHighlights, saveDetailPageComposition } = fp.detailLayoutSave;
</script>

<div class="space-y-4">
	<div
		class="flex flex-wrap gap-1 rounded-card border border-line bg-raised p-1"
		data-detail-layout-tabs
	>
		{#each detailLayoutModes as mode}
			<button
				type="button"
				class="inline-flex h-9 items-center rounded-control px-3 text-sm font-medium transition-colors {s.detailLayoutMode ===
				mode.key
					? 'rf-filled bg-primary text-on-primary'
					: 'text-fg-muted hover:bg-surface hover:text-fg'}"
				aria-current={s.detailLayoutMode === mode.key ? 'page' : undefined}
				data-detail-layout-tab={mode.key}
				onclick={() => (s.detailLayoutMode = mode.key)}>{$t(mode.labelKey)}</button
			>
		{/each}
	</div>

	<section
		class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode === 'header'
			? ''
			: 'hidden'}"
		data-detail-header-builder={s.selectedForm?.id ?? ''}
	>
		<div class="mb-3">
			<h3 class="text-base font-semibold text-fg">
				{$t('forms.detailHeader')}
			</h3>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.detailHeaderDescription')}
			</p>
		</div>

		<div class="grid gap-3">
			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.detailHeaderSubtitleField')}</span
				>

				<select
					value={s.detailHeaderDraft.subtitleField}
					disabled={s.detailHeaderSaving}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					data-detail-header-subtitle-field
					onchange={(event) =>
						setDetailHeaderDraft({
							subtitleField:
								event.currentTarget instanceof HTMLSelectElement ? event.currentTarget.value : ''
						})}
				>
					<option value="">{$t('forms.detailHeaderNoField')}</option>

					{#each s.detailVisibleFields as field}
						<option value={field.key}>{fieldLabel(field)}</option>
					{/each}
				</select>
			</label>

			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.detailHeaderBadgeField')}</span
				>

				<select
					value={s.detailHeaderDraft.badgeField}
					disabled={s.detailHeaderSaving}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					data-detail-header-badge-field
					onchange={(event) =>
						setDetailHeaderDraft({
							badgeField:
								event.currentTarget instanceof HTMLSelectElement ? event.currentTarget.value : ''
						})}
				>
					<option value="">{$t('forms.detailHeaderNoField')}</option>

					{#each s.detailVisibleFields as field}
						<option value={field.key}>{fieldLabel(field)}</option>
					{/each}
				</select>
			</label>
		</div>

		<div class="mt-3 flex justify-end">
			<span data-detail-header-save>
				<Button size="sm" loading={s.detailHeaderSaving} onclick={saveDetailHeader}
					>{$t('forms.saveDetailHeader')}</Button
				>
			</span>
		</div>
	</section>

	<section
		class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode === 'highlights'
			? ''
			: 'hidden'}"
		data-detail-highlights-builder={s.selectedForm?.id ?? ''}
	>
		<div class="mb-3 flex items-start justify-between gap-3">
			<div>
				<h3 class="text-base font-semibold text-fg">
					{$t('forms.detailHighlights')}
				</h3>

				<p class="mt-1 text-sm text-fg-muted">
					{$t('forms.detailHighlightsDescription')}
				</p>
			</div>

			{#if s.detailHighlightSaving}
				<span class="text-xs font-medium text-fg-muted">{$t('common.saving')}</span>
			{/if}
		</div>

		<div class="space-y-2">
			{#each s.detailVisibleFields as field}
				{@const highlightIndex = s.detailHighlightDraft.fields.indexOf(field.key)}
				{@const isChecked = highlightIndex >= 0}

				<div
					class="grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-card border border-line px-3 py-2"
					data-detail-highlight-field={field.key}
					data-detail-highlight-selected={`${field.key}:${isChecked}`}
				>
					<label class="flex h-10 w-10 items-center justify-center">
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-input accent-primary"
							checked={isChecked}
							disabled={s.detailHighlightSaving ||
								(!isChecked && s.detailHighlightDraft.fields.length >= 4)}
							aria-label={fieldLabel(field)}
							data-detail-highlight-toggle={field.key}
							onchange={(event) =>
								setDetailHighlightField(
									field.key,
									event.currentTarget instanceof HTMLInputElement
										? event.currentTarget.checked
										: false
								)}
						/>
					</label>

					<div class="min-w-0">
						<div class="truncate text-sm font-medium text-fg">
							{fieldLabel(field)}
						</div>

						<p class="mt-0.5 truncate font-mono text-xs text-fg-muted">
							{field.key}
						</p>
					</div>

					<div class="flex items-center gap-1">
						<button
							type="button"
							class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							disabled={!isChecked || highlightIndex === 0 || s.detailHighlightSaving}
							aria-label={$t('forms.moveHighlightUp')}
							title={$t('forms.moveHighlightUp')}
							data-detail-highlight-move-up={field.key}
							onclick={() => moveDetailHighlightField(field.key, -1)}
							><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
						>

						<button
							type="button"
							class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							disabled={!isChecked ||
								highlightIndex === s.detailHighlightDraft.fields.length - 1 ||
								s.detailHighlightSaving}
							aria-label={$t('forms.moveHighlightDown')}
							title={$t('forms.moveHighlightDown')}
							data-detail-highlight-move-down={field.key}
							onclick={() => moveDetailHighlightField(field.key, 1)}
							><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
						>
					</div>
				</div>
			{/each}
		</div>

		<div class="mt-3 flex items-center justify-between gap-3">
			<span class="text-xs text-fg-muted"
				>{$t('forms.detailHighlightsLimit', {
					values: { count: s.detailHighlightDraft.fields.length }
				})}</span
			>

			<span data-detail-highlights-save>
				<Button size="sm" loading={s.detailHighlightSaving} onclick={saveDetailHighlights}
					>{$t('forms.saveDetailHighlights')}</Button
				>
			</span>
		</div>
	</section>

	<section
		class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode === 'composition'
			? ''
			: 'hidden'}"
		data-detail-page-composition-builder={s.selectedForm?.id ?? ''}
	>
		<div class="mb-3">
			<h3 class="text-base font-semibold text-fg">
				{$t('forms.detailPageComposition')}
			</h3>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.detailPageCompositionDescription')}
			</p>
		</div>

		<label class="space-y-1">
			<span class="text-xs font-medium uppercase text-fg-muted"
				>{$t('forms.detailPageWidgetRegion')}</span
			>

			<select
				value={s.detailPageCompositionDraft.widgetRegion}
				disabled={s.detailPageCompositionSaving}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				data-detail-page-widget-region
				onchange={(event) =>
					(s.detailPageCompositionDraft = {
						...s.detailPageCompositionDraft,
						widgetRegion:
							event.currentTarget instanceof HTMLSelectElement
								? detailPageWidgetRegion(event.currentTarget.value)
								: 'sidebar'
					})}
			>
				<option value="sidebar">{$t('forms.detailPageWidgetRegionSidebar')}</option>
				<option value="below">{$t('forms.detailPageWidgetRegionBelow')}</option>
			</select>
		</label>

		<label class="mt-3 block space-y-1">
			<span class="text-xs font-medium uppercase text-fg-muted"
				>{$t('forms.detailPageFieldColumns')}</span
			>

			<select
				value={String(s.detailPageCompositionDraft.fieldColumns)}
				disabled={s.detailPageCompositionSaving}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				data-detail-page-field-columns
				onchange={(event) =>
					(s.detailPageCompositionDraft = {
						...s.detailPageCompositionDraft,
						fieldColumns:
							event.currentTarget instanceof HTMLSelectElement
								? detailPageFieldColumns(event.currentTarget.value)
								: 2
					})}
			>
				<option value="1">{$t('forms.detailPageFieldColumnsOne')}</option>
				<option value="2">{$t('forms.detailPageFieldColumnsTwo')}</option>
			</select>
		</label>

		<div class="mt-3 flex justify-end">
			<span data-detail-page-composition-save>
				<Button
					size="sm"
					loading={s.detailPageCompositionSaving}
					onclick={saveDetailPageComposition}>{$t('forms.saveDetailPageComposition')}</Button
				>
			</span>
		</div>
	</section>

	<DetailSectionsBuilder />

	<DetailWidgetsBuilder />
</div>
