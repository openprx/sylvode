<svelte:options runes />

<script lang="ts">
	/**
	 * The rule editor of the field inspector: built-in normalize ops, the `rules.validate` list
	 * (built-in rules with their parameters, CEL predicates, plugin references kept as stored) with
	 * a live check per rule, and a preview that runs a sample through the client rules and, on
	 * request, through the server pipeline against the unsaved schema. The host keys it by field,
	 * so another field starts with an empty preview.
	 */
	import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/svelte';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import type { FormFieldErrorItem, FormSchemaIssue } from '#lib/api/forms.js';
	import { fieldFromDraft } from '#lib/forms/designer-schema.js';
	import { errorMessage, type ErrorCode } from '#lib/forms/field-errors.js';
	import type { FieldDraft } from '#lib/forms/page-types.js';
	import {
		celRecord,
		fieldHints,
		isEmptyValue,
		type CheckContext
	} from '#lib/forms/record-checks.js';
	import {
		MAX_VALIDATE_RULES,
		addableRuleKinds,
		blankRuleDraft,
		checkRuleDraft,
		type RuleDraft
	} from '#lib/forms/rule-drafts.js';
	import type { RuleKind } from '#lib/forms/rules/index.js';
	import { FIELD_TYPES } from '#lib/forms/types/index.js';

	type ServerCheck = { errors: FormFieldErrorItem[] } | { failure: string };

	interface Props {
		field: FieldDraft;
		fields: FieldDraft[];
		issues?: FormSchemaIssue[];
		checkContext: () => CheckContext;
		onchange: (patch: Partial<FieldDraft>) => void;
		oncheckserver?: (values: Record<string, unknown>) => Promise<ServerCheck>;
	}

	let { field, fields, issues = [], checkContext, onchange, oncheckserver }: Props = $props();

	const lang = $derived($locale === 'en' ? 'en' : 'zh');
	const descriptor = $derived(FIELD_TYPES[field.type] ?? FIELD_TYPES.text);
	const addable = $derived(addableRuleKinds(field.type));
	const previewField = $derived(fieldFromDraft(field));
	const transformCount = $derived(
		Array.isArray(field.rulesExtra.transform) ? field.rulesExtra.transform.length : 0
	);
	let newKind = $state<RuleKind | ''>('');
	let sample = $state('');
	let recordText = $state('');
	let serverResult = $state<ServerCheck | null>(null);
	let serverChecking = $state(false);

	function message(error: ErrorCode): string {
		return errorMessage(previewField, error, $t, lang);
	}

	function setRules(rules: RuleDraft[]) {
		onchange({ rules });
		serverResult = null;
	}

	function updateRule(index: number, patch: Partial<RuleDraft>) {
		setRules(field.rules.map((rule, item) => (item === index ? { ...rule, ...patch } : rule)));
	}

	function moveRule(index: number, direction: -1 | 1) {
		const next = [...field.rules];
		const target = index + direction;
		if (target < 0 || target >= next.length) return;
		[next[index], next[target]] = [next[target], next[index]];
		setRules(next);
	}

	function addRule() {
		if (!newKind || field.rules.length >= MAX_VALIDATE_RULES) return;
		setRules([...field.rules, blankRuleDraft(newKind)]);
		newKind = '';
	}

	function hasOp(op: string): boolean {
		return field.normalize.some(
			(entry) => typeof entry === 'object' && entry !== null && 'op' in entry && entry.op === op
		);
	}

	function toggleOp(op: string, on: boolean) {
		const rest = field.normalize.filter(
			(entry) => !(typeof entry === 'object' && entry !== null && 'op' in entry && entry.op === op)
		);
		onchange({ normalize: on ? [...rest, { op }] : rest });
		serverResult = null;
	}

	function pluginLabel(rule: RuleDraft): string {
		const plugin = rule.plugin?.plugin as { package?: unknown; rule?: unknown } | undefined;
		return `${String(plugin?.package ?? '?')} / ${String(plugin?.rule ?? '?')}`;
	}

	function inputText(event: Event): string {
		const target = event.currentTarget;
		return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement
			? target.value
			: '';
	}

	function inputChecked(event: Event): boolean {
		return event.currentTarget instanceof HTMLInputElement && event.currentTarget.checked;
	}

	/** The sample as the record editor would send it for the type. */
	function sampleValue(text: string): unknown {
		if (text === '') return undefined;
		switch (field.type) {
			case 'integer':
			case 'rating':
			case 'progress':
				return /^-?\d+$/.test(text.trim()) ? Number.parseInt(text.trim(), 10) : text;
			case 'boolean':
				return text.trim() === 'true' ? true : text.trim() === 'false' ? false : text;
			case 'multi_select':
				return text
					.split(',')
					.map((item) => item.trim())
					.filter(Boolean);
			default:
				return text;
		}
	}

	type ParsedRecord = { ok: true; values: Record<string, unknown> } | { ok: false };

	const otherValues = $derived.by((): ParsedRecord => {
		if (!recordText.trim()) return { ok: true, values: {} };
		try {
			const parsed = JSON.parse(recordText) as unknown;
			return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
				? { ok: true, values: parsed as Record<string, unknown> }
				: { ok: false };
		} catch {
			return { ok: false };
		}
	});

	const previewValues = $derived.by(() => {
		const values = otherValues.ok ? { ...otherValues.values } : {};
		const value = sampleValue(sample);
		if (value === undefined) delete values[field.key];
		else values[field.key] = value;
		return values;
	});

	const clientVerdict = $derived.by((): ErrorCode[] => {
		const value = previewValues[field.key];
		const required: ErrorCode[] =
			field.required && isEmptyValue(value) ? [{ code: 'required', rule: null, params: null }] : [];
		const record = celRecord(fields.map(fieldFromDraft), previewValues);
		return [...required, ...fieldHints(previewField, value, record, checkContext())];
	});

	async function checkOnServer() {
		if (!oncheckserver) return;
		serverChecking = true;
		serverResult = await oncheckserver(previewValues);
		serverChecking = false;
	}

	const serverFieldErrors = $derived(
		serverResult && 'errors' in serverResult
			? serverResult.errors.filter((error) => error.field === field.key)
			: []
	);
	const serverOtherCount = $derived(
		serverResult && 'errors' in serverResult
			? serverResult.errors.length - serverFieldErrors.length
			: 0
	);

	function codes(errors: ErrorCode[]): string {
		return errors.map((error) => error.code).join(' ');
	}

	const controlClass =
		'w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg';
</script>

<div class="space-y-3" data-field-rules={field.key}>
	{#if issues.length > 0}
		<ul class="space-y-1 rounded-control bg-danger-soft p-2 text-xs text-danger" role="alert">
			{#each issues as issue, index (index)}
				<li data-field-schema-issue={issue.code}>
					<span class="font-mono">{issue.path || '/'}</span>
					{message({ code: issue.code })}
				</li>
			{/each}
		</ul>
	{/if}

	{#if descriptor.normalizeOps.length > 0}
		<fieldset class="space-y-1">
			<legend class="text-xs font-medium text-fg-muted">
				{$t('lowcode.forms.rules.normalize')}
			</legend>
			<div class="flex flex-wrap gap-3">
				{#each descriptor.normalizeOps as op (op)}
					<label class="flex items-center gap-1.5 text-sm text-fg">
						<input
							type="checkbox"
							class="h-4 w-4 rounded-control border-input"
							checked={hasOp(op)}
							onchange={(event) => toggleOp(op, inputChecked(event))}
							data-normalize-op={op}
						/>
						{$t(`lowcode.forms.rules.ops.${op}`)}
					</label>
				{/each}
			</div>
		</fieldset>
	{/if}

	{#if descriptor.allowedRules.length === 0}
		<p class="text-xs text-fg-muted">{$t('lowcode.forms.rules.noneForType')}</p>
	{/if}

	{#each field.rules as rule, index (index)}
		{@const problem = checkRuleDraft(rule, field.type)}
		<div
			class="space-y-2 rounded-control border p-2 {problem ? 'border-danger' : 'border-line'}"
			data-rule-index={index}
			data-rule-kind={rule.kind}
		>
			<div class="flex items-center justify-between gap-2">
				<span class="text-xs font-semibold text-fg">
					{$t(`lowcode.forms.rules.kinds.${rule.kind}`)}
				</span>
				<span class="flex items-center gap-1">
					<button
						type="button"
						class="rounded-control p-1 text-fg-muted hover:bg-surface disabled:opacity-40"
						aria-label={$t('lowcode.forms.rules.moveUp')}
						disabled={index === 0}
						onclick={() => moveRule(index, -1)}
					>
						<ArrowUp class="h-3.5 w-3.5" aria-hidden="true" />
					</button>
					<button
						type="button"
						class="rounded-control p-1 text-fg-muted hover:bg-surface disabled:opacity-40"
						aria-label={$t('lowcode.forms.rules.moveDown')}
						disabled={index === field.rules.length - 1}
						onclick={() => moveRule(index, 1)}
					>
						<ArrowDown class="h-3.5 w-3.5" aria-hidden="true" />
					</button>
					<button
						type="button"
						class="rounded-control p-1 text-danger hover:bg-danger-soft"
						aria-label={$t('lowcode.forms.rules.remove')}
						onclick={() => setRules(field.rules.filter((_, item) => item !== index))}
						data-rule-remove={index}
					>
						<Trash2 class="h-3.5 w-3.5" aria-hidden="true" />
					</button>
				</span>
			</div>

			{#if rule.kind === 'cel'}
				<textarea
					class="{controlClass} min-h-16 font-mono"
					value={rule.value}
					placeholder="size(value) <= 16"
					aria-label={$t('lowcode.forms.rules.celExpression')}
					oninput={(event) => updateRule(index, { value: inputText(event) })}
					data-rule-cel={index}></textarea>
				<label class="flex items-center gap-1.5 text-xs text-fg">
					<input
						type="checkbox"
						class="h-4 w-4 rounded-control border-input"
						checked={rule.runOnEmpty}
						onchange={(event) => updateRule(index, { runOnEmpty: inputChecked(event) })}
					/>
					{$t('lowcode.forms.rules.runOnEmpty')}
				</label>
			{:else if rule.kind === 'plugin'}
				<p class="font-mono text-xs text-fg-muted">{pluginLabel(rule)}</p>
			{:else if rule.kind === 'min' || rule.kind === 'max'}
				<input
					class={controlClass}
					type={field.type === 'date'
						? 'date'
						: field.type === 'datetime'
							? 'datetime-local'
							: 'text'}
					value={rule.value}
					aria-label={$t('lowcode.forms.rules.bound')}
					oninput={(event) => updateRule(index, { value: inputText(event) })}
					data-rule-value={index}
				/>
			{:else if rule.kind === 'min_length' || rule.kind === 'max_length'}
				<input
					class={controlClass}
					type="number"
					min="0"
					value={rule.value}
					aria-label={$t('lowcode.forms.rules.length')}
					oninput={(event) => updateRule(index, { value: inputText(event) })}
					data-rule-value={index}
				/>
			{:else if rule.kind === 'pattern'}
				<input
					class="{controlClass} font-mono"
					value={rule.value}
					placeholder="^[A-Z]+$"
					aria-label={$t('lowcode.forms.rules.pattern')}
					oninput={(event) => updateRule(index, { value: inputText(event) })}
					data-rule-value={index}
				/>
				<label class="flex items-center gap-1.5 text-xs text-fg">
					<input
						type="checkbox"
						class="h-4 w-4 rounded-control border-input"
						checked={rule.caseInsensitive}
						onchange={(event) => updateRule(index, { caseInsensitive: inputChecked(event) })}
					/>
					{$t('lowcode.forms.rules.caseInsensitive')}
				</label>
			{:else if rule.kind === 'enum'}
				<textarea
					class="{controlClass} min-h-16"
					value={rule.valuesText}
					aria-label={$t('lowcode.forms.rules.enumValues')}
					placeholder={$t('lowcode.forms.rules.enumValues')}
					oninput={(event) => updateRule(index, { valuesText: inputText(event) })}></textarea>
			{:else if rule.kind === 'url'}
				<div class="flex gap-3">
					{#each ['https', 'http'] as scheme (scheme)}
						<label class="flex items-center gap-1.5 text-xs text-fg">
							<input
								type="checkbox"
								class="h-4 w-4 rounded-control border-input"
								checked={rule.schemes.includes(scheme)}
								onchange={(event) =>
									updateRule(index, {
										schemes: inputChecked(event)
											? ['https', 'http'].filter(
													(item) => item === scheme || rule.schemes.includes(item)
												)
											: rule.schemes.filter((item) => item !== scheme)
									})}
							/>
							{scheme}
						</label>
					{/each}
				</div>
			{/if}

			<details class="text-xs">
				<summary class="cursor-pointer text-fg-muted">{$t('lowcode.forms.rules.failure')}</summary>
				<div class="mt-2 grid gap-2">
					<input
						class="{controlClass} font-mono"
						value={rule.code}
						placeholder={$t('lowcode.forms.rules.codePlaceholder')}
						aria-label={$t('lowcode.forms.rules.code')}
						oninput={(event) => updateRule(index, { code: inputText(event) })}
						data-rule-code={index}
					/>
					{#each ['en', 'zh'] as language (language)}
						<input
							class={controlClass}
							value={rule.message[language] ?? ''}
							placeholder={$t(`lowcode.forms.rules.message.${language}`)}
							aria-label={$t(`lowcode.forms.rules.message.${language}`)}
							oninput={(event) =>
								updateRule(index, { message: { ...rule.message, [language]: inputText(event) } })}
						/>
					{/each}
				</div>
			</details>

			{#if problem}
				<p class="text-xs text-danger" data-rule-problem={problem.code}>
					{message({ code: problem.code })}
				</p>
			{/if}
		</div>
	{/each}

	{#if addable.length > 0}
		<div class="flex items-center gap-2">
			<select
				class="{controlClass} flex-1"
				bind:value={newKind}
				aria-label={$t('lowcode.forms.rules.add')}
				data-rule-add-kind
			>
				<option value="">{$t('lowcode.forms.rules.choose')}</option>
				{#each addable as kind (kind)}
					<option value={kind}>{$t(`lowcode.forms.rules.kinds.${kind}`)}</option>
				{/each}
			</select>
			<Button
				size="sm"
				variant="secondary"
				disabled={!newKind || field.rules.length >= MAX_VALIDATE_RULES}
				onclick={addRule}
			>
				<Plus class="mr-1 h-4 w-4" aria-hidden="true" />
				{$t('lowcode.forms.rules.add')}
			</Button>
		</div>
	{/if}

	{#if transformCount > 0}
		<p class="text-xs text-fg-muted">
			{$t('lowcode.forms.rules.transformKept', { values: { count: transformCount } })}
		</p>
	{/if}

	{#if descriptor.allowedRules.length > 0}
		<div class="space-y-2 rounded-control bg-surface p-2" data-rule-preview={field.key}>
			<p class="text-xs font-semibold text-fg">{$t('lowcode.forms.rules.preview')}</p>
			<input
				class={controlClass}
				value={sample}
				placeholder={$t('lowcode.forms.rules.sample')}
				aria-label={$t('lowcode.forms.rules.sample')}
				oninput={(event) => {
					sample = inputText(event);
					serverResult = null;
				}}
				data-rule-preview-sample
			/>
			<textarea
				class="{controlClass} min-h-12 font-mono text-xs"
				value={recordText}
				placeholder={$t('lowcode.forms.rules.otherFields')}
				aria-label={$t('lowcode.forms.rules.otherFields')}
				oninput={(event) => {
					recordText = inputText(event);
					serverResult = null;
				}}
				data-rule-preview-record></textarea>
			{#if !otherValues.ok}
				<p class="text-xs text-danger">{$t('lowcode.forms.rules.otherFieldsInvalid')}</p>
			{/if}
			<div class="text-xs" data-rule-preview-client={codes(clientVerdict) || 'pass'}>
				<span class="font-medium text-fg">{$t('lowcode.forms.rules.clientVerdict')}</span>
				{#if clientVerdict.length === 0}
					<span class="text-success">{$t('lowcode.forms.rules.pass')}</span>
				{:else}
					<ul class="text-warning">
						{#each clientVerdict as error, index (index)}
							<li><span class="font-mono">{error.code}</span> · {message(error)}</li>
						{/each}
					</ul>
				{/if}
			</div>
			{#if oncheckserver}
				<Button
					size="sm"
					variant="secondary"
					loading={serverChecking}
					disabled={!otherValues.ok}
					onclick={() => void checkOnServer()}
				>
					{$t('lowcode.forms.rules.checkOnServer')}
				</Button>
			{/if}
			{#if serverResult}
				{#if 'failure' in serverResult}
					<p class="text-xs text-danger" data-rule-preview-server="failure">
						{serverResult.failure}
					</p>
				{:else}
					<div class="text-xs" data-rule-preview-server={codes(serverFieldErrors) || 'pass'}>
						<span class="font-medium text-fg">{$t('lowcode.forms.rules.serverVerdict')}</span>
						{#if serverFieldErrors.length === 0}
							<span class="text-success">{$t('lowcode.forms.rules.pass')}</span>
						{:else}
							<ul class="text-danger">
								{#each serverFieldErrors as error, index (index)}
									<li><span class="font-mono">{error.code}</span> · {message(error)}</li>
								{/each}
							</ul>
						{/if}
						{#if serverOtherCount > 0}
							<p class="text-fg-muted">
								{$t('lowcode.forms.rules.serverOther', { values: { count: serverOtherCount } })}
							</p>
						{/if}
					</div>
				{/if}
			{/if}
		</div>
	{/if}
</div>
