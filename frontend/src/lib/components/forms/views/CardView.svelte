<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import {
		ArrowDown,
		Copy,
		FileDown,
		FileText,
		Filter,
		MoreVertical,
		TableProperties,
		Trash2
	} from '@lucide/svelte';
	import { type FormRecord } from '#lib/api/forms.js';
	import OptionValueBadges from '#lib/components/forms/OptionValueBadges.svelte';
	import {
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { formatDate, selectValue } from '#lib/forms/record-format.js';
	import {
		cardCoverImageClass,
		cardCoverObjectClass,
		kanbanMoveOptions,
		viewCardFieldLimit
	} from '#lib/forms/view-config.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleDeleteRecord } = fp.records;
	const { selectRecord } = fp.recordPanel;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const {
		copyRecordId,
		exportCardRecordDetails,
		exportCardGroupRecords,
		updateCardRecordSelectField,
		cardGroupCollapsed,
		toggleCardGroup,
		cardGroupFilterKey,
		copyCardGroupRecordIds,
		focusCardGroupRecords,
		canApplyCardGroupFilter,
		applyCardGroupFilter,
		cardDisplayFields,
		viewCardBadgeField,
		cardTitle,
		cardSubtitle,
		cardBadge,
		cardCoverUrl
	} = fp.viewCards;
</script>

	{#snippet cardRecord(record: FormRecord)}
		{@const badgeField = viewCardBadgeField()}
		{@const badge = cardBadge(record)}
		{@const coverUrl = cardCoverUrl(record)}

		<article
			class={s.selectedCardLayout === 'gallery'
				? 'overflow-hidden rounded-card border border-line bg-raised ring-1 ring-transparent transition-shadow hover:shadow-md'
				: s.selectedCardLayout === 'compact' || s.selectedCardLayout === 'list'
					? 'overflow-visible rounded-control border border-line bg-raised'
					: 'overflow-hidden rounded-card border border-line bg-raised'}
			data-card-copied={s.copiedCardRecordId === record.id
				? record.id
				: undefined}
			data-card-exported={s.exportedCardRecordId === record.id
				? record.id
				: undefined}
			data-card-compact-row={s.selectedCardLayout === 'compact'
				? record.id
				: undefined}
			data-card-list-row={s.selectedCardLayout === 'list' ? record.id : undefined}
			data-card-cover-aspect={s.selectedCardCoverAspect}
			data-card-cover-fit={s.selectedCardCoverFit}
			data-card-cover-position={s.selectedCardCoverPosition}
		>
			{#if coverUrl && s.selectedCardLayout !== 'compact' && s.selectedCardLayout !== 'list' && s.selectedCardCoverPosition === 'top'}
				<img
					src={coverUrl}
					alt={cardTitle(record)}
					class={cardCoverImageClass(
						s.selectedCardLayout,
						s.selectedCardCoverAspect,
						s.selectedCardCoverFit
					)}
					data-card-cover={record.id}
					data-card-cover-aspect={s.selectedCardCoverAspect}
					data-card-cover-fit={s.selectedCardCoverFit}
					data-card-cover-position={s.selectedCardCoverPosition}
				/>
			{/if}

			<div
				class={s.selectedCardLayout === 'compact' ||
				s.selectedCardLayout === 'list'
					? 'flex items-center justify-between gap-3 px-3 py-2'
					: s.selectedCardCoverPosition === 'left' && coverUrl
						? 'grid grid-cols-[7rem_minmax(0,1fr)_auto] items-start gap-3 p-4 pb-0'
						: 'flex items-start justify-between gap-3 p-4 pb-0'}
			>
				{#if coverUrl && s.selectedCardLayout !== 'compact' && s.selectedCardLayout !== 'list' && s.selectedCardCoverPosition === 'left'}
					<img
						src={coverUrl}
						alt={cardTitle(record)}
						class={`h-24 w-28 shrink-0 rounded-control ${cardCoverObjectClass(s.selectedCardCoverFit)}`}
						data-card-cover={record.id}
						data-card-cover-aspect={s.selectedCardCoverAspect}
						data-card-cover-fit={s.selectedCardCoverFit}
						data-card-cover-position={s.selectedCardCoverPosition}
					/>
				{/if}

				{#if coverUrl && s.selectedCardLayout === 'compact' && s.selectedCardCoverPosition !== 'hidden'}
					<img
						src={coverUrl}
						alt={cardTitle(record)}
						class={`h-12 w-12 shrink-0 rounded-control ${cardCoverObjectClass(s.selectedCardCoverFit)}`}
						data-card-cover={record.id}
						data-card-cover-aspect={s.selectedCardCoverAspect}
						data-card-cover-fit={s.selectedCardCoverFit}
						data-card-cover-position={s.selectedCardCoverPosition}
					/>
				{/if}

				<button
					type="button"
					class="min-w-0 text-left"
					onclick={() => selectRecord(record)}
					aria-label={`${$t('forms.openRecord')}: ${record.title}`}
				>
					<span
						class={s.selectedCardLayout === 'gallery'
							? 'block truncate text-base font-semibold text-fg'
							: s.selectedCardLayout === 'compact' ||
								  s.selectedCardLayout === 'list'
								? 'block truncate text-sm font-semibold text-fg'
								: 'block truncate font-medium text-fg'}
						data-card-title={record.id}>{cardTitle(record)}</span
					>

					<span
						class="mt-1 block truncate text-xs text-fg-muted"
						data-card-subtitle={record.id}>{cardSubtitle(record)}</span
					>

					{#if s.selectedCardShowTimestamps}
						<span
							class="mt-1 flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-fg-muted"
							data-card-timestamps={record.id}
						>
							<span data-card-created-at={record.id}
								>{$t('forms.cardCreatedAt')}: {formatDate(
									record.created_at
								)}</span
							>
							<span data-card-updated-at={record.id}
								>{$t('forms.cardUpdatedAt')}: {formatDate(
									record.updated_at
								)}</span
							>
						</span>
					{/if}

					{#if s.selectedCardShowRecordId}
						<span
							class="mt-1 block truncate font-mono text-[11px] text-fg-muted"
							data-card-record-id={record.id}
							>{$t('forms.cardRecordId')}: {record.id}</span
						>
					{/if}
				</button>

				<div class="flex shrink-0 items-start gap-2">
					{#if badge}
						<span class="max-w-32 truncate" data-card-badge={record.id}
							><OptionValueBadges field={badgeField} value={badge} /></span
						>
					{/if}

					<details class="group relative shrink-0" data-card-menu={record.id}>
						<summary
							class="inline-flex cursor-pointer list-none items-center rounded-control p-1.5 text-fg-muted hover:bg-surface hover:text-fg"
							aria-label={$t('forms.cardActions')}
							><MoreVertical class="h-4 w-4" aria-hidden="true" /></summary
						>

						<div
							class="absolute right-0 z-20 mt-2 w-40 overflow-hidden rounded-float border border-line bg-overlay py-1 text-sm shadow-overlay"
						>
							<button
								type="button"
								class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
								data-card-action={`${record.id}:open`}
								onclick={() => selectRecord(record)}
							>
								<FileText class="h-4 w-4" aria-hidden="true" />
								{$t('forms.openRecord')}
							</button>

							<button
								type="button"
								class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
								data-card-action={`${record.id}:copy-id`}
								onclick={() => copyRecordId(record)}
							>
								<Copy class="h-4 w-4" aria-hidden="true" />
								{$t('forms.copyRecordId')}
							</button>

							<button
								type="button"
								class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
								data-card-action={`${record.id}:export-record`}
								onclick={() => exportCardRecordDetails(record)}
							>
								<FileDown class="h-4 w-4" aria-hidden="true" />
								{$t('forms.exportRecord')}
							</button>

							<button
								type="button"
								class="flex w-full items-center gap-2 px-3 py-2 text-left text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
								disabled={s.deletingRecordId === record.id}
								data-card-action={`${record.id}:delete`}
								onclick={() => handleDeleteRecord(record)}
							>
								<Trash2 class="h-4 w-4" aria-hidden="true" />
								{$t('forms.deleteRecord')}
							</button>
						</div>
					</details>
				</div>
			</div>

			<dl
				class={s.selectedCardLayout === 'compact'
					? 'grid gap-x-4 gap-y-1 border-t border-line px-3 py-2 sm:grid-cols-2 lg:grid-cols-3'
					: s.selectedCardLayout === 'list'
						? 'grid gap-x-4 gap-y-1 border-t border-line px-3 py-2 sm:grid-cols-3 xl:grid-cols-5'
						: 'mt-4 space-y-2 px-4'}
			>
				{#each cardDisplayFields(record) as field}
					<div
						class={s.selectedCardLayout === 'compact' ||
						s.selectedCardLayout === 'list'
							? 'min-w-0 text-xs'
							: 'grid grid-cols-[7rem_minmax(0,1fr)] gap-3 text-sm'}
						data-card-field={`${record.id}:${field.key}`}
					>
						<dt
							class={s.selectedCardLayout === 'compact' ||
							s.selectedCardLayout === 'list'
								? 'flex min-w-0 flex-wrap items-center gap-1 text-[10px] font-medium uppercase text-fg-muted'
								: 'flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted'}
						>
							<span class="min-w-0 truncate">{fieldLabel(field)}</span>

							{#if memberFieldHidden(field)}
								<span
									class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
									data-card-field-permission-hidden={`${record.id}:${field.key}`}
									>{$t('forms.fieldHiddenBadge')}</span
								>
							{/if}

							{#if memberFieldLocked(field)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
									data-card-field-permission-locked={`${record.id}:${field.key}`}
									>{$t('forms.fieldLockedBadge')}</span
								>
							{/if}

							{#if recordFieldHiddenByCondition(field, record)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
									data-card-field-conditional-hidden={`${record.id}:${field.key}`}
									>{$t('forms.fieldConditionHiddenBadge')}</span
								>
							{/if}

							{#if recordFieldReadOnlyByCondition(field, record)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
									data-card-field-conditional-readonly={`${record.id}:${field.key}`}
									>{$t('forms.fieldConditionReadOnlyBadge')}</span
								>
							{/if}
						</dt>

						<dd class="truncate text-fg">
							<OptionValueBadges {field} value={record.values[field.key]} />
						</dd>
					</div>
				{/each}
			</dl>

			{#if s.cardEditableSelectFields.length > 0}
				<div class="mt-4 space-y-2 border-t border-line px-4 py-3">
					{#each s.cardEditableSelectFields as field}
						<label class="block space-y-1">
							<span
								class="flex flex-wrap items-center gap-1.5 text-xs font-medium text-fg-muted"
							>
								<span>{fieldLabel(field)}</span>

								{#if memberFieldHidden(field)}
									<span
										class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
										data-card-select-permission-hidden={`${record.id}:${field.key}`}
										>{$t('forms.fieldHiddenBadge')}</span
									>
								{/if}

								{#if memberFieldLocked(field)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
										data-card-select-permission-locked={`${record.id}:${field.key}`}
										>{$t('forms.fieldLockedBadge')}</span
									>
								{/if}

								{#if recordFieldHiddenByCondition(field, record)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
										data-card-select-conditional-hidden={`${record.id}:${field.key}`}
										>{$t('forms.fieldConditionHiddenBadge')}</span
									>
								{/if}

								{#if recordFieldReadOnlyByCondition(field, record)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
										data-card-select-conditional-readonly={`${record.id}:${field.key}`}
										>{$t('forms.fieldConditionReadOnlyBadge')}</span
									>
								{/if}
							</span>

							<select
								value={String(record.values[field.key] ?? '')}
								disabled={s.updatingCardRecordId === record.id}
								aria-label={`${$t('forms.cardUpdateField')} ${fieldLabel(field)}`}
								data-card-select={`${record.id}:${field.key}`}
								onchange={(event) =>
									updateCardRecordSelectField(record, field, selectValue(event))}
								class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
							>
								{#each kanbanMoveOptions(field) as option}
									<option value={option}>{option}</option>
								{/each}
							</select>
						</label>
					{/each}
				</div>
			{/if}
		</article>
	{/snippet}

	<div
		class={s.cardGroupField
			? 'space-y-4 p-4'
			: s.selectedCardLayout === 'gallery'
				? 'grid gap-4 p-4 md:grid-cols-2 2xl:grid-cols-4'
				: s.selectedCardLayout === 'compact' || s.selectedCardLayout === 'list'
					? 'space-y-2 p-4'
					: 'grid gap-3 p-4 md:grid-cols-2 xl:grid-cols-3'}
		data-view-renderer="card"
		data-card-layout={s.selectedCardLayout}
		data-card-field-limit={viewCardFieldLimit(
			s.selectedView?.config?.card_field_limit
		)}
		data-card-hide-empty-fields={s.selectedCardHideEmptyFields}
		data-card-show-record-id={s.selectedCardShowRecordId}
		data-card-group-field={s.cardGroupField?.key}
	>
		{#if s.cardGroupField}
			{#each s.cardGroups as group}
				{@const collapsed = cardGroupCollapsed(group)}

				<section
					class="rounded-card border border-line bg-nav p-3"
					data-card-group={group.label}
					data-card-group-collapsed={collapsed ? 'true' : 'false'}
					data-record-count={group.records.length}
					data-card-group-record-ids-copied={s.copiedCardGroupRecordIdsKey ===
					cardGroupFilterKey(group)
						? 'true'
						: undefined}
					data-card-group-records-exported={s.exportedCardGroupRecordsKey ===
					cardGroupFilterKey(group)
						? 'true'
						: undefined}
				>
					<div class="mb-3 flex items-center justify-between gap-3">
						<div class="min-w-0">
							<div class="text-xs font-medium uppercase text-fg-muted">
								{$t('forms.viewGroupBy', {
									values: { field: fieldLabel(s.cardGroupField) }
								})}
							</div>

							<h4 class="truncate text-sm font-semibold text-fg">
								{group.label}
							</h4>
						</div>

						<div class="flex shrink-0 items-center gap-2">
							<span class="rounded bg-raised px-2 py-0.5 text-xs text-fg-muted"
								>{$t('forms.groupRecordCount', {
									values: { count: group.records.length }
								})}</span
							>

							<button
								type="button"
								class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
								aria-label={$t('forms.copyCardGroupRecordIds')}
								title={$t('forms.copyCardGroupRecordIds')}
								disabled={group.records.length === 0}
								onclick={() => copyCardGroupRecordIds(group)}
								data-card-group-copy-record-ids={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
								><Copy class="h-4 w-4" aria-hidden="true" /></button
							>

							<button
								type="button"
								class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
								aria-label={$t('forms.exportCardGroupRecords')}
								title={$t('forms.exportCardGroupRecords')}
								disabled={group.records.length === 0}
								onclick={() => exportCardGroupRecords(group)}
								data-card-group-export-records={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
								><FileDown class="h-4 w-4" aria-hidden="true" /></button
							>

							<button
								type="button"
								class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
								aria-label={$t('forms.focusCardGroupRecords')}
								title={$t('forms.focusCardGroupRecords')}
								disabled={group.records.length === 0}
								onclick={() => focusCardGroupRecords(group)}
								data-card-group-focus-records={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
								><TableProperties class="h-4 w-4" aria-hidden="true" /></button
							>

							<button
								type="button"
								class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
								aria-label={$t('forms.applyCardGroupFilter')}
								title={$t('forms.applyCardGroupFilter')}
								disabled={!canApplyCardGroupFilter(group)}
								onclick={() => applyCardGroupFilter(group)}
								data-card-group-apply-filter={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
								data-card-group-filter-applied={s.appliedCardGroupFilterKey ===
								cardGroupFilterKey(group)
									? 'true'
									: undefined}
								><Filter class="h-4 w-4" aria-hidden="true" /></button
							>

							<button
								type="button"
								class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg"
								aria-label={collapsed
									? $t('forms.expandCardGroup')
									: $t('forms.collapseCardGroup')}
								onclick={() => toggleCardGroup(group)}
								data-card-group-toggle={group.label}
							>
								<ArrowDown
									class="h-4 w-4 transition-transform {collapsed
										? '-rotate-90'
										: ''}"
									aria-hidden="true"
								/>
							</button>
						</div>
					</div>

					{#if collapsed}
						<p
							class="rounded-card border border-dashed border-line bg-raised px-3 py-2 text-xs text-fg-muted"
						>
							{$t('forms.cardGroupCollapsed', {
								values: { count: group.records.length }
							})}
						</p>
					{:else}
						<div
							class={s.selectedCardLayout === 'gallery'
								? 'grid gap-4 md:grid-cols-2 2xl:grid-cols-4'
								: s.selectedCardLayout === 'compact' ||
									  s.selectedCardLayout === 'list'
									? 'space-y-2'
									: 'grid gap-3 md:grid-cols-2 xl:grid-cols-3'}
						>
							{#each group.records as record}
								{@render cardRecord(record)}
							{/each}
						</div>
					{/if}
				</section>
			{/each}
		{:else}
			{#each s.viewRecords as record}
				{@render cardRecord(record)}
			{/each}
		{/if}
	</div>
