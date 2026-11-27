<script lang="ts">
	import {
		AlertTriangle,
		ArrowDown,
		ArrowUp,
		Copy,
		GripVertical,
		PanelLeftOpen,
		Plus,
		Save,
		ShieldCheck,
		SlidersHorizontal,
		Trash2,
		X
	} from '@lucide/svelte';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import type {
		FormFieldDependency,
		FormFieldErrorItem,
		FormFieldType,
		FormFieldUsage,
		FormSchemaIssue,
		UniversalForm
	} from '#lib/api/forms.js';
	import FieldRulesEditor from '#lib/components/forms/FieldRulesEditor.svelte';
	import { errorMessage } from '#lib/forms/field-errors.js';
	import type { FieldDraft } from '#lib/forms/page-types.js';
	import type { CheckContext } from '#lib/forms/record-checks.js';
	import { checkDefaultCel, type DefaultMode } from '#lib/forms/rule-drafts.js';

	interface Props {
		form: UniversalForm;
		fieldDrafts: FieldDraft[];
		fieldTypeOptions: FormFieldType[];
		selectedIndex: number;
		selectedFieldUsage?: FormFieldUsage | null;
		selectedFieldDependencies?: FormFieldDependency[];
		hiddenFieldKeys?: string[];
		lockedFieldKeys?: string[];
		usageLoading?: boolean;
		saving?: boolean;
		/** Issues of the last refused schema save, by field key (`''` for the schema). */
		schemaIssues?: Record<string, FormSchemaIssue[]>;
		checkContext: () => CheckContext;
		onCheckOnServer?: (
			values: Record<string, unknown>
		) => Promise<{ errors: FormFieldErrorItem[] } | { failure: string }>;
		onSelectField?: (index: number) => void;
		onAppendField?: (type?: FormFieldType) => void;
		onUpdateField?: (index: number, patch: Partial<FieldDraft>) => void;
		onDuplicateField?: (index: number) => void;
		onRemoveField?: (index: number) => void;
		onMoveField?: (index: number, direction: -1 | 1) => void;
		onSave?: () => void;
	}

	let {
		form,
		fieldDrafts,
		fieldTypeOptions,
		selectedIndex,
		selectedFieldUsage = null,
		selectedFieldDependencies = [],
		hiddenFieldKeys = [],
		lockedFieldKeys = [],
		usageLoading = false,
		saving = false,
		schemaIssues = {},
		checkContext,
		onCheckOnServer,
		onSelectField,
		onAppendField,
		onUpdateField,
		onDuplicateField,
		onRemoveField,
		onMoveField,
		onSave
	}: Props = $props();

	const selectedField = $derived(fieldDrafts[selectedIndex] ?? fieldDrafts[0] ?? null);
	const selectedValueCount = $derived(selectedFieldUsage?.value_count ?? 0);
	const selectedIndexedCount = $derived(selectedFieldUsage?.indexed_count ?? 0);
	const selectedDependencyCount = $derived(selectedFieldDependencies.length);
	const selectedBlockingDependencyCount = $derived(
		selectedFieldDependencies.filter((dependency) => dependency.blocking).length
	);
	const selectedSafetyWarning = $derived(selectedValueCount > 0 || selectedDependencyCount > 0);
	const hiddenFieldKeySet = $derived(new Set(hiddenFieldKeys));
	const lockedFieldKeySet = $derived(new Set(lockedFieldKeys));
	const selectedFieldHidden = $derived(
		Boolean(selectedField?.key && hiddenFieldKeySet.has(selectedField.key))
	);
	const selectedFieldLocked = $derived(
		Boolean(selectedField?.key && lockedFieldKeySet.has(selectedField.key))
	);
	let mobilePanel = $state<'library' | 'inspector' | null>(null);
	const lang = $derived($locale === 'en' ? 'en' : 'zh');
	const defaultCelProblem = $derived(
		selectedField?.defaultMode === 'cel' && selectedField.defaultCel.trim()
			? checkDefaultCel(selectedField.defaultCel)
			: null
	);

	const fieldGroups = $derived([
		{
			label: $t('forms.fieldGroups.basic'),
			types: [
				'text',
				'phone',
				'email',
				'address',
				'location',
				'textarea',
				'number',
				'integer',
				'date',
				'datetime',
				'boolean'
			] as FormFieldType[]
		},
		{
			label: $t('forms.fieldGroups.choice'),
			types: ['single_select', 'multi_select'] as FormFieldType[]
		},
		{
			label: $t('forms.fieldGroups.business'),
			types: [
				'amount',
				'rating',
				'progress',
				'scan',
				'signature',
				'autonumber',
				'member',
				'attachment',
				'image'
			] as FormFieldType[]
		},
		{
			label: $t('forms.fieldGroups.relation'),
			types: ['relation', 'child_table'] as FormFieldType[]
		},
		{
			label: $t('forms.fieldGroups.automation'),
			types: ['formula', 'ai_summary'] as FormFieldType[]
		}
	]);

	function updateSelected(patch: Partial<FieldDraft>) {
		if (!selectedField) return;
		onUpdateField?.(selectedIndex, patch);
	}

	function handleFieldKeydown(event: KeyboardEvent, index: number) {
		if (event.key !== 'Enter' && event.key !== ' ') return;
		event.preventDefault();
		onSelectField?.(index);
	}

	function fieldHidden(field: FieldDraft): boolean {
		return Boolean(field.key && hiddenFieldKeySet.has(field.key));
	}

	function fieldLocked(field: FieldDraft): boolean {
		return Boolean(field.key && lockedFieldKeySet.has(field.key));
	}
</script>

{#if mobilePanel}
	<button
		type="button"
		class="fixed inset-0 z-30 bg-scrim md:hidden"
		aria-label="关闭遮罩"
		onclick={() => {
			mobilePanel = null;
		}}
	></button>
{/if}

<section
	class="grid min-w-0 items-start gap-4 xl:grid-cols-[200px_minmax(0,1fr)_300px] 2xl:grid-cols-[240px_minmax(0,1fr)_340px]"
>
	<div class="grid grid-cols-2 gap-2 md:hidden" data-mobile-field-design-toolbar>
		<Button
			size="sm"
			variant="secondary"
			onclick={() => {
				mobilePanel = 'library';
			}}
		>
			<PanelLeftOpen class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.fieldLibrary')}
		</Button>
		<Button
			size="sm"
			variant="secondary"
			onclick={() => {
				mobilePanel = 'inspector';
			}}
		>
			<SlidersHorizontal class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.properties')}
		</Button>
	</div>

	<aside
		class="{mobilePanel === 'library'
			? 'fixed inset-x-3 bottom-3 top-20 z-40 flex max-h-[calc(100vh-6rem)] flex-col overflow-hidden shadow-overlay'
			: 'hidden'} rounded-float border border-overlay-line bg-overlay md:static md:z-auto md:flex md:max-h-none md:flex-col md:overflow-visible md:shadow-none xl:sticky xl:top-4 xl:max-h-[calc(100vh-7rem)] xl:overflow-hidden"
		data-mobile-field-library-drawer={mobilePanel === 'library' ? 'open' : 'closed'}
	>
		<div class="flex items-center justify-between gap-3 border-b border-line px-4 py-3">
			<h3 class="text-sm font-semibold text-fg">
				{$t('forms.fieldLibrary')}
			</h3>
			<button
				type="button"
				class="rounded-control p-1.5 text-fg-muted hover:bg-surface md:hidden"
				aria-label={$t('common.close')}
				onclick={() => {
					mobilePanel = null;
				}}
			>
				<X class="h-4 w-4" aria-hidden="true" />
			</button>
		</div>
		<div class="space-y-4 overflow-y-auto p-3 xl:max-h-[calc(100vh-11rem)]">
			{#each fieldGroups as group, idx (idx)}
				<div>
					<p class="px-1 text-xs font-medium uppercase text-fg-muted">
						{group.label}
					</p>
					<div class="mt-2 grid gap-1">
						{#each group.types as type, idx (idx)}
							<button
								type="button"
								class="flex min-h-10 items-center justify-between gap-2 rounded-row px-3 text-left text-sm text-fg hover:bg-surface"
								data-field-library-type={type}
								onclick={() => {
									onAppendField?.(type);
									mobilePanel = null;
								}}
							>
								<span>{$t(`forms.fieldTypes.${type}`)}</span>
								<span
									class="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-card border border-line text-fg-muted"
								>
									<Plus class="h-4 w-4" aria-hidden="true" />
								</span>
							</button>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	</aside>

	<section class="min-w-0 rounded-card border border-line bg-raised" data-field-designer-canvas>
		<div
			class="flex flex-col gap-3 border-b border-line px-4 py-3 sm:flex-row sm:items-center sm:justify-between"
		>
			<div>
				<h3 class="text-base font-semibold text-fg">{form.name}</h3>
				<p class="mt-0.5 font-mono text-xs text-fg-muted">
					{form.key} · v{form.schema_version} · {$t('forms.fieldCount', {
						values: { count: fieldDrafts.length }
					})}
				</p>
			</div>
			<Button variant="primary" size="sm" loading={saving} onclick={onSave}>
				<Save class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.saveDesign')}
			</Button>
		</div>
		{#if (schemaIssues['']?.length ?? 0) > 0}
			<ul
				class="space-y-1 border-b border-line bg-danger-soft px-4 py-2 text-xs text-danger"
				role="alert"
			>
				{#each schemaIssues[''] ?? [] as issue, index (index)}
					<li data-schema-issue={issue.code}>
						<span class="font-mono">{issue.path || '/'}</span>
						{errorMessage(undefined, issue, $t, lang)}
					</li>
				{/each}
			</ul>
		{/if}
		<div class="min-h-[520px] bg-surface p-4">
			{#if fieldDrafts.length === 0}
				<div class="rounded-card border border-dashed border-line bg-raised p-8 text-center">
					<p class="text-sm text-fg-muted">{$t('forms.emptyCanvas')}</p>
				</div>
			{:else}
				<div class="mx-auto grid max-w-3xl gap-2">
					{#each fieldDrafts as field, index (index)}
						<div
							role="button"
							tabindex="0"
							class="group grid min-h-16 grid-cols-[24px_minmax(0,1fr)_auto] items-center gap-3 rounded-card border bg-raised px-3 py-2 text-left transition-colors {index ===
							selectedIndex
								? 'border-fg ring-2 ring-line'
								: 'border-line hover:border-line-strong'}"
							data-designer-field-key={field.key}
							data-designer-field-type={field.type}
							onclick={() => onSelectField?.(index)}
							onkeydown={(event) => handleFieldKeydown(event, index)}
						>
							<GripVertical class="h-4 w-4 text-fg-muted" aria-hidden="true" />
							<span class="min-w-0">
								<span
									class="flex min-w-0 flex-wrap items-center gap-1.5 text-sm font-medium text-fg"
								>
									<span class="truncate">
										{field.label || field.key || $t('forms.unnamedField')}
										{#if field.required}<span class="text-danger">*</span>{/if}
									</span>
									{#if fieldHidden(field)}
										<span
											class="rounded-[var(--radius-sm)] bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
											data-designer-field-permission-hidden={field.key}
										>
											{$t('forms.fieldHiddenBadge')}
										</span>
									{/if}
									{#if fieldLocked(field)}
										<span
											class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-designer-field-permission-locked={field.key}
										>
											{$t('forms.fieldLockedBadge')}
										</span>
									{/if}
								</span>
								<span class="mt-0.5 block truncate font-mono text-xs text-fg-muted">
									{field.key || 'field_key'} · {$t(`forms.fieldTypes.${field.type}`)}
								</span>
							</span>
							<span class="flex items-center gap-1">
								<button
									type="button"
									class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:text-fg-muted"
									disabled={index === 0}
									onclick={(event) => {
										event.stopPropagation();
										onMoveField?.(index, -1);
									}}
									aria-label={$t('forms.moveFieldUp')}
									data-designer-field-move-up={field.key || index}
								>
									<ArrowUp class="h-3.5 w-3.5" aria-hidden="true" />
								</button>
								<button
									type="button"
									class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:text-fg-muted"
									disabled={index === fieldDrafts.length - 1}
									onclick={(event) => {
										event.stopPropagation();
										onMoveField?.(index, 1);
									}}
									aria-label={$t('forms.moveFieldDown')}
									data-designer-field-move-down={field.key || index}
								>
									<ArrowDown class="h-3.5 w-3.5" aria-hidden="true" />
								</button>
							</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</section>

	<aside
		class="{mobilePanel === 'inspector'
			? 'fixed inset-x-3 bottom-3 top-20 z-40 flex max-h-[calc(100vh-6rem)] flex-col overflow-hidden shadow-overlay'
			: 'hidden'} rounded-float border border-overlay-line bg-overlay md:static md:z-auto md:flex md:max-h-none md:flex-col md:overflow-visible md:shadow-none xl:sticky xl:top-4 xl:max-h-[calc(100vh-7rem)] xl:overflow-hidden"
		data-mobile-field-inspector-drawer={mobilePanel === 'inspector' ? 'open' : 'closed'}
	>
		<div class="flex items-center justify-between gap-3 border-b border-line px-4 py-3">
			<div class="min-w-0">
				<p class="text-sm font-semibold text-fg">
					{$t('forms.properties')}
				</p>
				{#if selectedField}
					<p class="mt-0.5 truncate font-mono text-xs text-fg-muted">
						{selectedField.key || 'field_key'} · {$t(`forms.fieldTypes.${selectedField.type}`)}
					</p>
				{/if}
			</div>
			<button
				type="button"
				class="rounded-control p-1.5 text-fg-muted hover:bg-surface md:hidden"
				aria-label={$t('common.close')}
				onclick={() => {
					mobilePanel = null;
				}}
			>
				<X class="h-4 w-4" aria-hidden="true" />
			</button>
		</div>
		{#if selectedField}
			<div class="space-y-4 overflow-y-auto p-4 xl:max-h-[calc(100vh-11rem)]">
				{#if selectedFieldHidden || selectedFieldLocked}
					<div class="flex flex-wrap items-center gap-1.5">
						{#if selectedFieldHidden}
							<span
								class="rounded-[var(--radius-sm)] bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
								data-designer-property-permission-hidden={selectedField.key}
							>
								{$t('forms.fieldHiddenBadge')}
							</span>
						{/if}
						{#if selectedFieldLocked}
							<span
								class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
								data-designer-property-permission-locked={selectedField.key}
							>
								{$t('forms.fieldLockedBadge')}
							</span>
						{/if}
					</div>
				{/if}
				<label class="space-y-1">
					<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldLabel')}</span>
					<input
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						value={selectedField.label}
						oninput={(event) =>
							updateSelected({
								label:
									event.currentTarget instanceof HTMLInputElement ? event.currentTarget.value : ''
							})}
					/>
				</label>
				<label class="space-y-1">
					<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldKey')}</span>
					<input
						class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
						value={selectedField.key}
						oninput={(event) =>
							updateSelected({
								key:
									event.currentTarget instanceof HTMLInputElement ? event.currentTarget.value : ''
							})}
					/>
				</label>
				<label class="space-y-1">
					<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldType')}</span>
					<select
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						value={selectedField.type}
						onchange={(event) =>
							updateSelected({
								type:
									event.currentTarget instanceof HTMLSelectElement
										? (event.currentTarget.value as FormFieldType)
										: 'text'
							})}
					>
						{#each fieldTypeOptions as type, idx (idx)}
							<option value={type}>{$t(`forms.fieldTypes.${type}`)}</option>
						{/each}
					</select>
				</label>
				<label class="flex items-center gap-2 text-sm text-fg">
					<input
						type="checkbox"
						class="h-4 w-4 rounded-control border-input text-fg"
						checked={selectedField.required}
						onchange={(event) =>
							updateSelected({
								required:
									event.currentTarget instanceof HTMLInputElement
										? event.currentTarget.checked
										: false
							})}
					/>
					{$t('forms.required')}
				</label>
				{#if selectedField.type === 'single_select' || selectedField.type === 'multi_select'}
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionList')}</span>
						<textarea
							class="min-h-28 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							value={selectedField.optionsText}
							oninput={(event) =>
								updateSelected({
									optionsText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}></textarea>
					</label>
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionColors')}</span>
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
							value={selectedField.optionColorsText}
							placeholder={$t('forms.optionColorsPlaceholder')}
							oninput={(event) =>
								updateSelected({
									optionColorsText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}
							data-option-colors-input={selectedField.key}></textarea>
					</label>
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionDisabled')}</span>
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
							value={selectedField.optionDisabledText}
							placeholder={$t('forms.optionDisabledPlaceholder')}
							oninput={(event) =>
								updateSelected({
									optionDisabledText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}
							data-option-disabled-input={selectedField.key}></textarea>
					</label>
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionArchived')}</span>
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
							value={selectedField.optionArchivedText}
							placeholder={$t('forms.optionArchivedPlaceholder')}
							oninput={(event) =>
								updateSelected({
									optionArchivedText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}
							data-option-archived-input={selectedField.key}
							data-option-archived-state={selectedField.optionArchivedText}></textarea>
					</label>
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionDescriptions')}</span>
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
							value={selectedField.optionDescriptionsText}
							placeholder={$t('forms.optionDescriptionsPlaceholder')}
							oninput={(event) =>
								updateSelected({
									optionDescriptionsText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}
							data-option-descriptions-input={selectedField.key}></textarea>
					</label>
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.optionGroups')}</span>
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
							value={selectedField.optionGroupsText}
							placeholder={$t('forms.optionGroupsPlaceholder')}
							oninput={(event) =>
								updateSelected({
									optionGroupsText:
										event.currentTarget instanceof HTMLTextAreaElement
											? event.currentTarget.value
											: ''
								})}
							data-option-groups-input={selectedField.key}></textarea>
					</label>
				{/if}
				{#if selectedField.type === 'amount'}
					<div class="grid grid-cols-2 gap-3">
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.currency')}</span>
							<input
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm uppercase text-fg"
								maxlength="3"
								value={selectedField.currency}
								oninput={(event) =>
									updateSelected({
										currency:
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.value
												: 'CNY'
									})}
							/>
						</label>
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.scale')}</span>
							<input
								type="number"
								min="0"
								max="8"
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={selectedField.scale}
								oninput={(event) =>
									updateSelected({
										scale:
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.value
												: '2'
									})}
							/>
						</label>
					</div>
				{/if}
				{#if ['formula', 'amount', 'number', 'integer'].includes(selectedField.type)}
					<div class="space-y-3 rounded-card border border-line p-3">
						<p class="text-xs font-semibold uppercase text-fg-muted">
							{$t('forms.formulaConfig')}
						</p>
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.formulaOp')}</span>
							<select
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={selectedField.formulaOp}
								onchange={(event) =>
									updateSelected({
										formulaOp:
											event.currentTarget instanceof HTMLSelectElement
												? event.currentTarget.value
												: 'add'
									})}
							>
								<option value="add">add</option>
								<option value="sum">sum</option>
								<option value="subtract">subtract</option>
								<option value="multiply">multiply</option>
								<option value="divide">divide</option>
								<option value="min">min</option>
								<option value="max">max</option>
								<option value="count">count</option>
								<option value="child_sum">child_sum</option>
								<option value="child_count">child_count</option>
								<option value="child_min">child_min</option>
								<option value="child_max">child_max</option>
							</select>
						</label>
						{#if selectedField.formulaOp.startsWith('child_')}
							<div class="grid grid-cols-2 gap-3">
								<label class="space-y-1">
									<span class="text-xs font-medium text-fg-muted">{$t('forms.relationKey')}</span>
									<input
										class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
										value={selectedField.formulaRelationKey}
										oninput={(event) =>
											updateSelected({
												formulaRelationKey:
													event.currentTarget instanceof HTMLInputElement
														? event.currentTarget.value
														: ''
											})}
									/>
								</label>
								<label class="space-y-1">
									<span class="text-xs font-medium text-fg-muted">{$t('forms.childField')}</span>
									<input
										class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
										value={selectedField.formulaChildField}
										oninput={(event) =>
											updateSelected({
												formulaChildField:
													event.currentTarget instanceof HTMLInputElement
														? event.currentTarget.value
														: ''
											})}
									/>
								</label>
							</div>
						{:else}
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.formulaArgs')}</span>
								<textarea
									class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.formulaArgsText}
									oninput={(event) =>
										updateSelected({
											formulaArgsText:
												event.currentTarget instanceof HTMLTextAreaElement
													? event.currentTarget.value
													: ''
										})}></textarea>
							</label>
						{/if}
						{#if selectedField.type === 'formula'}
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.scale')}</span>
								<input
									type="number"
									min="0"
									max="8"
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.formulaScale}
									oninput={(event) =>
										updateSelected({
											formulaScale:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: '2'
										})}
								/>
							</label>
						{/if}
					</div>
				{/if}
				{#if selectedField.type === 'relation' || selectedField.type === 'child_table'}
					<div class="space-y-3 rounded-card border border-line p-3">
						<p class="text-xs font-semibold uppercase text-fg-muted">
							{$t('forms.relationConfig')}
						</p>
						<div class="grid grid-cols-2 gap-3">
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.targetForm')}</span>
								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.relationFormKey}
									oninput={(event) =>
										updateSelected({
											relationFormKey:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.relationType')}</span>
								<select
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.type === 'child_table'
										? 'parent_child'
										: selectedField.relationType}
									disabled={selectedField.type === 'child_table'}
									onchange={(event) =>
										updateSelected({
											relationType:
												event.currentTarget instanceof HTMLSelectElement
													? event.currentTarget.value
													: 'relation'
										})}
								>
									<option value="relation">relation</option>
									<option value="parent_child">parent_child</option>
									<option value="external_relation">external_relation</option>
								</select>
							</label>
						</div>
						<div class="grid grid-cols-2 gap-3">
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.relationKey')}</span>
								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.relationKey}
									oninput={(event) =>
										updateSelected({
											relationKey:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.displayField')}</span>
								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.relationDisplayField}
									oninput={(event) =>
										updateSelected({
											relationDisplayField:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							</label>
						</div>
					</div>
				{/if}
				{#if selectedField.type === 'attachment' || selectedField.type === 'image'}
					<div class="space-y-3 rounded-card border border-line p-3">
						<p class="text-xs font-semibold uppercase text-fg-muted">
							{$t('forms.attachmentConfig')}
						</p>
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.acceptedTypes')}</span>
							<textarea
								class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
								value={selectedField.attachmentAcceptText}
								oninput={(event) =>
									updateSelected({
										attachmentAcceptText:
											event.currentTarget instanceof HTMLTextAreaElement
												? event.currentTarget.value
												: ''
									})}></textarea>
						</label>
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.maxSizeMb')}</span>
							<input
								type="number"
								min="1"
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={selectedField.attachmentMaxSizeMb}
								oninput={(event) =>
									updateSelected({
										attachmentMaxSizeMb:
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.value
												: ''
									})}
							/>
						</label>
						<div class="grid grid-cols-2 gap-2">
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									class="h-4 w-4 rounded-control border-input text-fg"
									checked={selectedField.attachmentMultiple}
									onchange={(event) =>
										updateSelected({
											attachmentMultiple:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: false
										})}
								/>
								{$t('forms.multipleFiles')}
							</label>
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									class="h-4 w-4 rounded-control border-input text-fg"
									checked={selectedField.attachmentPreview}
									onchange={(event) =>
										updateSelected({
											attachmentPreview:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: false
										})}
								/>
								{$t('forms.previewFiles')}
							</label>
						</div>
						<div class="grid gap-2 md:grid-cols-3">
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.thumbnailFormat')}</span>
								<select
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.attachmentThumbnailFormat}
									data-attachment-thumbnail-format={selectedField.key}
									onchange={(event) =>
										updateSelected({
											attachmentThumbnailFormat:
												event.currentTarget instanceof HTMLSelectElement
													? event.currentTarget.value
													: 'png'
										})}
								>
									<option value="png">PNG</option>
									<option value="jpeg">JPEG</option>
									<option value="webp">WebP</option>
								</select>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.previewFormat')}</span>
								<select
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.attachmentPreviewFormat}
									data-attachment-preview-format={selectedField.key}
									onchange={(event) =>
										updateSelected({
											attachmentPreviewFormat:
												event.currentTarget instanceof HTMLSelectElement
													? event.currentTarget.value
													: 'png'
										})}
								>
									<option value="png">PNG</option>
									<option value="jpeg">JPEG</option>
									<option value="webp">WebP</option>
								</select>
							</label>
							<label class="space-y-1 md:col-span-3">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.variantPolicies')}</span>
								<textarea
									class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.attachmentVariantsText}
									placeholder={$t('forms.variantPolicyPlaceholder')}
									data-attachment-variants={selectedField.key}
									oninput={(event) =>
										updateSelected({
											attachmentVariantsText:
												event.currentTarget instanceof HTMLTextAreaElement
													? event.currentTarget.value
													: ''
										})}></textarea>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.storagePolicy')}</span>
								<select
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.attachmentStoragePolicy}
									data-attachment-storage-policy={selectedField.key}
									onchange={(event) =>
										updateSelected({
											attachmentStoragePolicy:
												event.currentTarget instanceof HTMLSelectElement
													? event.currentTarget.value
													: 'local'
										})}
								>
									<option value="local">{$t('forms.storagePolicyLocal')}</option>
									<option value="private">{$t('forms.storagePolicyPrivate')}</option>
									<option value="external">{$t('forms.storagePolicyExternal')}</option>
								</select>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.retentionDays')}</span>
								<input
									type="number"
									min="1"
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.attachmentRetentionDays}
									data-attachment-retention-days={selectedField.key}
									oninput={(event) =>
										updateSelected({
											attachmentRetentionDays:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted"
									>{$t('forms.signedUrlTtlMinutes')}</span
								>
								<input
									type="number"
									min="1"
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.attachmentSignedUrlTtlMinutes}
									data-attachment-signed-url-ttl={selectedField.key}
									oninput={(event) =>
										updateSelected({
											attachmentSignedUrlTtlMinutes:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							</label>
						</div>
					</div>
				{/if}
				<details
					class="rounded-card border border-line bg-raised"
					data-field-inspector-section="input-behavior"
				>
					<summary
						class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs font-semibold uppercase text-fg-muted marker:hidden"
					>
						<span>{$t('forms.inputBehavior')}</span>
						<span class="font-normal normal-case text-fg-muted">
							{$t('forms.fieldInspectorAdvanced')}
						</span>
					</summary>
					<div class="space-y-3 border-t border-line p-3">
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.placeholder')}</span>
							<input
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={selectedField.placeholder}
								oninput={(event) =>
									updateSelected({
										placeholder:
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.value
												: ''
									})}
							/>
						</label>
						<label class="space-y-1">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.helpText')}</span>
							<textarea
								class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={selectedField.helpText}
								oninput={(event) =>
									updateSelected({
										helpText:
											event.currentTarget instanceof HTMLTextAreaElement
												? event.currentTarget.value
												: ''
									})}></textarea>
						</label>
						<div class="space-y-1" data-field-default={selectedField.key}>
							<span class="flex items-center justify-between gap-2">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.defaultValue')}</span>
								<select
									class="rounded-control border border-input bg-field px-2 py-1 text-xs text-fg"
									value={selectedField.defaultMode}
									aria-label={$t('lowcode.forms.designer.defaultMode')}
									onchange={(event) =>
										updateSelected({
											defaultMode: (event.currentTarget instanceof HTMLSelectElement
												? event.currentTarget.value
												: 'value') as DefaultMode
										})}
									data-field-default-mode
								>
									<option value="value">{$t('lowcode.forms.designer.defaultFixed')}</option>
									<option value="cel">{$t('lowcode.forms.designer.defaultCel')}</option>
								</select>
							</span>
							{#if selectedField.defaultMode === 'cel'}
								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.defaultCel}
									placeholder="now"
									aria-label={$t('lowcode.forms.designer.defaultCel')}
									oninput={(event) =>
										updateSelected({
											defaultCel:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
									data-field-default-cel
								/>
								{#if defaultCelProblem}
									<p class="text-xs text-danger" data-field-default-problem={defaultCelProblem}>
										{errorMessage(undefined, { code: defaultCelProblem }, $t, lang)}
									</p>
								{:else}
									<p class="text-xs text-fg-muted">{$t('lowcode.forms.designer.defaultCelHint')}</p>
								{/if}
							{:else}
								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									value={selectedField.defaultValue}
									aria-label={$t('forms.defaultValue')}
									oninput={(event) =>
										updateSelected({
											defaultValue:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.value
													: ''
										})}
								/>
							{/if}
						</div>
						<div class="grid grid-cols-2 gap-2">
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									class="h-4 w-4 rounded-control border-input text-fg"
									checked={selectedField.listVisible}
									onchange={(event) =>
										updateSelected({
											listVisible:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: true
										})}
								/>
								{$t('forms.listVisible')}
							</label>
							<label class="flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									class="h-4 w-4 rounded-control border-input text-fg"
									checked={selectedField.detailVisible}
									onchange={(event) =>
										updateSelected({
											detailVisible:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: true
										})}
								/>
								{$t('forms.detailVisible')}
							</label>
							<label class="col-span-2 flex items-center gap-2 text-sm text-fg">
								<input
									type="checkbox"
									class="h-4 w-4 rounded-control border-input text-fg"
									checked={selectedField.readOnly}
									onchange={(event) =>
										updateSelected({
											readOnly:
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: false
										})}
								/>
								{$t('forms.readOnly')}
							</label>
						</div>
						<div class="grid gap-3 md:grid-cols-2">
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">
									{$t('forms.conditionalHiddenWhen')}
								</span>
								<textarea
									class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.conditionalHiddenWhenText}
									placeholder={$t('forms.conditionalRulePlaceholder')}
									oninput={(event) =>
										updateSelected({
											conditionalHiddenWhenText:
												event.currentTarget instanceof HTMLTextAreaElement
													? event.currentTarget.value
													: ''
										})}
									data-conditional-hidden-input={selectedField.key}></textarea>
							</label>
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">
									{$t('forms.conditionalReadOnlyWhen')}
								</span>
								<textarea
									class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
									value={selectedField.conditionalReadOnlyWhenText}
									placeholder={$t('forms.conditionalRulePlaceholder')}
									oninput={(event) =>
										updateSelected({
											conditionalReadOnlyWhenText:
												event.currentTarget instanceof HTMLTextAreaElement
													? event.currentTarget.value
													: ''
										})}
									data-conditional-readonly-input={selectedField.key}></textarea>
							</label>
						</div>
					</div>
				</details>
				<details
					class="rounded-card border border-line bg-raised"
					data-field-inspector-section="validation"
					open={selectedField.rules.length > 0 ||
						(schemaIssues[selectedField.key]?.length ?? 0) > 0}
				>
					<summary
						class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs font-semibold uppercase text-fg-muted marker:hidden"
					>
						<span>{$t('forms.validationRules')}</span>
						<span class="font-normal normal-case text-fg-muted">
							{$t('forms.fieldInspectorAdvanced')}
						</span>
					</summary>
					<div class="border-t border-line p-3">
						{#key selectedField.key}
							<FieldRulesEditor
								field={selectedField}
								fields={fieldDrafts}
								issues={schemaIssues[selectedField.key] ?? []}
								{checkContext}
								onchange={updateSelected}
								oncheckserver={onCheckOnServer}
							/>
						{/key}
					</div>
				</details>
				<details
					data-field-safety="selected"
					data-field-safety-values={selectedValueCount}
					data-field-safety-dependencies={selectedDependencyCount}
					data-field-safety-blocking={selectedBlockingDependencyCount}
					open={selectedSafetyWarning || usageLoading}
					class="rounded-card border p-3 text-sm {selectedBlockingDependencyCount > 0
						? 'border-danger bg-danger-soft text-danger'
						: selectedSafetyWarning
							? 'border-line bg-warning-soft text-warning'
							: 'border-line bg-success-soft text-success'}"
				>
					<summary
						class="flex cursor-pointer list-none items-center justify-between gap-3 marker:hidden"
					>
						<span class="flex items-center gap-2 font-medium">
							{#if selectedSafetyWarning || usageLoading}
								<AlertTriangle class="h-4 w-4 shrink-0" aria-hidden="true" />
							{:else}
								<ShieldCheck class="h-4 w-4 shrink-0" aria-hidden="true" />
							{/if}
							<span>{$t('forms.fieldSafetyTitle')}</span>
						</span>
						<span class="text-xs">
							{selectedSafetyWarning
								? $t('forms.fieldInspectorReview')
								: $t('forms.fieldInspectorClear')}
						</span>
					</summary>
					{#if usageLoading}
						<p class="mt-2 text-xs">{$t('forms.fieldSafetyLoading')}</p>
					{:else if selectedSafetyWarning}
						<div class="mt-2 grid grid-cols-2 gap-2 text-xs">
							<div>
								<span class="block">{$t('forms.valueUsage')}</span>
								<span class="font-mono font-semibold">{selectedValueCount}</span>
							</div>
							<div>
								<span class="block">{$t('forms.indexedUsage')}</span>
								<span class="font-mono font-semibold">{selectedIndexedCount}</span>
							</div>
							<div>
								<span class="block">{$t('forms.dependencyUsage')}</span>
								<span class="font-mono font-semibold">{selectedDependencyCount}</span>
							</div>
							<div>
								<span class="block">{$t('forms.blockingDependencies')}</span>
								<span class="font-mono font-semibold">{selectedBlockingDependencyCount}</span>
							</div>
						</div>
						{#if selectedFieldDependencies.length > 0}
							<ul class="mt-2 space-y-1 text-xs">
								{#each selectedFieldDependencies.slice(0, 4) as dependency, idx (idx)}
									<li class="break-words">
										<span class="font-medium">{dependency.source_type}</span>
										{#if dependency.source_key}
											<span class="font-mono">/{dependency.source_key}</span>
										{/if}
										<span> · {dependency.reason}</span>
									</li>
								{/each}
							</ul>
						{/if}
					{:else}
						<p class="mt-2 text-xs">{$t('forms.fieldSafetyClear')}</p>
					{/if}
				</details>
				<div class="grid gap-2 pt-2">
					<Button size="md" variant="secondary" onclick={() => onDuplicateField?.(selectedIndex)}>
						<Copy class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.duplicateField')}
					</Button>
					<Button
						size="md"
						variant="danger-quiet"
						disabled={fieldDrafts.length === 1 || selectedBlockingDependencyCount > 0}
						onclick={() => onRemoveField?.(selectedIndex)}
					>
						<Trash2 class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.removeField')}
					</Button>
				</div>
			</div>
		{:else}
			<div class="p-4 text-sm text-fg-muted">
				{$t('forms.noFieldSelected')}
			</div>
		{/if}
	</aside>
</section>
