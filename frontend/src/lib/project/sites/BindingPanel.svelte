<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { UniversalForm } from '#lib/api/forms.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import SidePanel from '#lib/ui/side-panel/SidePanel.svelte';
	import { defaultWidth } from '#lib/ui/side-panel/side-panel.js';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import {
		bindingFields,
		draftOf,
		emptyDraft,
		isBindingKey,
		issuesAt,
		parseMaxPerHour,
		placeIssues,
		projectionOf,
		withKey,
		withReadField,
		type BindingDraft,
		type BindingField,
		type IssueSection,
		type PlacedIssue,
		type ReadDraft,
		type TypeOutlets
	} from './view';
	import type { SiteBinding } from './wire';

	/**
	 * Adds or changes one binding in a side panel beside the list. The form is chosen among the
	 * project forms the person can view; each audience gets a field list drawn from the form's
	 * schema and the field types' site outlets. The server checks the projection again and its
	 * refusals are placed at the field they name.
	 */
	let {
		siteId,
		published,
		forms,
		outlets,
		takenKeys,
		binding,
		onclose,
		onsaved
	}: {
		siteId: string;
		/** Saving changes what visitors see now. */
		published: boolean;
		forms: UniversalForm[];
		outlets: TypeOutlets;
		/** Keys of the other bindings; a new binding may not reuse one (a save would replace it). */
		takenKeys: string[];
		/** The binding to change; absent for a new one. */
		binding?: SiteBinding;
		onclose: () => void;
		onsaved: () => void;
	} = $props();

	/** The panel is keyed by binding, so the draft is seeded once from the binding it opened with. */
	const seed = (): BindingDraft => (binding ? draftOf(binding) : emptyDraft());
	let draft = $state<BindingDraft>(seed());
	let width = $state(defaultWidth(typeof window === 'undefined' ? 1280 : window.innerWidth));
	let busy = $state(false);
	let keyError = $state('');
	let formError = $state('');
	let limitError = $state('');
	let formProblem = $state('');
	let issues = $state.raw<PlacedIssue[]>([]);

	const form = $derived(forms.find((item) => item.id === draft.formId) ?? null);
	const fields = $derived<BindingField[]>(form ? bindingFields(form.schema, outlets) : []);
	const formOptions = $derived(
		forms.map((item) => ({ value: item.id, label: item.name, meta: item.key }))
	);
	const formGone = $derived(!!binding && !form);

	function pickForm(id: string | null): void {
		if (id === draft.formId) return;
		// Field lists belong to one form's schema.
		draft = { ...emptyDraft(), key: draft.key, formId: id };
		issues = [];
		formProblem = '';
	}

	function fieldIssues(name: IssueSection, key: string): string[] {
		return issuesAt(issues, name, key).map((issue) => issueText(issue.code));
	}

	function issueText(code: string): string {
		const key = `sites.bindings.issue.${code}`;
		const text = $t(key);
		return text === key ? code : text;
	}

	function setRead(name: 'visitorRead' | 'operatorRead', next: ReadDraft): void {
		draft = { ...draft, [name]: next };
	}

	async function submit(): Promise<void> {
		keyError = '';
		formError = '';
		limitError = '';
		formProblem = '';
		issues = [];
		if (!binding) {
			if (!isBindingKey(draft.key)) keyError = $t('sites.errors.binding_key_invalid');
			else if (takenKeys.includes(draft.key)) keyError = $t('sites.errors.binding_key_taken');
		}
		if (!draft.formId || !form) formProblem = $t('sites.bindings.formRequired');
		if (draft.submit.on && parseMaxPerHour(draft.submit.maxPerHour) === null)
			limitError = $t('sites.bindings.issue.limit_invalid');
		if (keyError || formProblem || limitError || !draft.formId) return;

		const projection = projectionOf(draft, fields);
		busy = true;
		const result = await sitesApi.putBinding(siteId, draft.key, {
			form_id: draft.formId,
			projection
		});
		busy = false;
		if (result.code === 0) {
			onsaved();
			return;
		}
		if (result.error_code === 'projection_invalid') {
			issues = placeIssues(result.details, projection);
			if (issues.length === 0) formError = errorText(result, $t);
		} else if (result.error_code === 'binding_key_invalid') keyError = errorText(result, $t);
		else if (result.error_code === 'not_found') formProblem = $t('sites.bindings.formNotUsable');
		else formError = errorText(result, $t);
	}
</script>

{#snippet readFields(name: 'visitorRead' | 'operatorRead', read: ReadDraft)}
	{#each fields.filter((field) => field.read) as field (field.key)}
		{@const on = read.fields.includes(field.key)}
		<div class="row">
			<Checkbox
				label={field.column ? $t(`sites.bindings.column.${field.key}`) : field.label}
				checked={on}
				onchange={(event) =>
					setRead(name, withReadField(read, field.key, event.currentTarget.checked))}
				data-testid={`${name}-${field.key}`}
			/>
			{#if on && field.filter}
				<Checkbox
					label={$t('sites.bindings.filterable')}
					checked={read.filter.includes(field.key)}
					onchange={(event) =>
						setRead(name, {
							...read,
							filter: withKey(read.filter, field.key, event.currentTarget.checked)
						})}
				/>
			{/if}
			{#if on && field.sort}
				<Checkbox
					label={$t('sites.bindings.sortable')}
					checked={read.sort.includes(field.key)}
					onchange={(event) =>
						setRead(name, {
							...read,
							sort: withKey(read.sort, field.key, event.currentTarget.checked)
						})}
				/>
			{/if}
		</div>
		{#each fieldIssues(name, field.key) as text (text)}<p class="issue" role="alert">
				{text}
			</p>{/each}
	{/each}
	{@render blocked(fields.filter((field) => !field.read))}
{/snippet}

{#snippet blocked(list: BindingField[])}
	{#if list.length > 0}
		<p class="muted" data-testid="blocked-fields">
			{$t('sites.bindings.cannotShow', {
				values: { fields: list.map((field) => field.label).join(', ') }
			})}
		</p>
	{/if}
{/snippet}

{#snippet sectionIssues(name: IssueSection)}
	{#each issuesAt(issues, name).filter((issue) => issue.part !== 'limit') as issue (issue.part + issue.code)}
		<p class="issue" role="alert">{issueText(issue.code)}</p>
	{/each}
{/snippet}

<SidePanel
	title={binding
		? $t('sites.bindings.editTitle', { values: { key: binding.binding_key } })
		: $t('sites.bindings.addTitle')}
	status="ready"
	{width}
	onresize={(px) => (width = px)}
	{onclose}
>
	<div class="panel" data-testid="binding-panel">
		{#if published}
			<TipBar tone="warning" data-testid="published-tip">{$t('sites.bindings.publishedTip')}</TipBar
			>
		{/if}
		{#if formGone}
			<TipBar tone="warning">{$t('sites.bindings.formGone')}</TipBar>
		{/if}

		<TextField
			label={$t('sites.bindings.key')}
			value={draft.key}
			oninput={(event) => {
				draft = { ...draft, key: event.currentTarget.value };
				keyError = '';
			}}
			helper={$t('sites.bindings.keyHelper')}
			error={keyError || undefined}
			disabled={!!binding}
			maxlength={40}
			autocomplete="off"
			data-testid="binding-key"
		/>
		<Select
			label={$t('sites.bindings.form')}
			value={draft.formId}
			options={formOptions}
			placeholder={$t('sites.bindings.formPlaceholder')}
			helper={forms.length === 0 ? $t('sites.bindings.noForms') : undefined}
			error={formProblem || undefined}
			searchable="auto"
			onchange={pickForm}
			testId="binding-form"
		/>

		{#if form}
			<fieldset class="group" data-testid="section-submit">
				<legend>{$t('sites.bindings.submitTitle')}</legend>
				<Checkbox
					label={$t('sites.bindings.submitOn')}
					checked={draft.submit.on}
					onchange={(event) =>
						(draft = { ...draft, submit: { ...draft.submit, on: event.currentTarget.checked } })}
					data-testid="submit-on"
				/>
				{#if draft.submit.on}
					{#each fields.filter((field) => field.submit) as field (field.key)}
						<div class="row">
							<Checkbox
								label={field.label}
								checked={draft.submit.fields.includes(field.key)}
								onchange={(event) =>
									(draft = {
										...draft,
										submit: {
											...draft.submit,
											fields: withKey(draft.submit.fields, field.key, event.currentTarget.checked)
										}
									})}
								data-testid={`submit-${field.key}`}
							/>
						</div>
						{#each fieldIssues('visitorSubmit', field.key) as text (text)}
							<p class="issue" role="alert">{text}</p>
						{/each}
					{/each}
					{@render blocked(fields.filter((field) => !field.submit && !field.column))}
					<TextField
						label={$t('sites.bindings.maxPerHour')}
						value={draft.submit.maxPerHour}
						oninput={(event) => {
							draft = {
								...draft,
								submit: { ...draft.submit, maxPerHour: event.currentTarget.value }
							};
							limitError = '';
						}}
						helper={$t('sites.bindings.maxPerHourHelper')}
						error={limitError ||
							issuesAt(issues, 'visitorSubmit')
								.filter((issue) => issue.part === 'limit')
								.map((issue) => issueText(issue.code))[0] ||
							undefined}
						inputmode="numeric"
						autocomplete="off"
						data-testid="max-per-hour"
					/>
					<p class="muted">{$t('sites.bindings.captchaOff')}</p>
					{@render sectionIssues('visitorSubmit')}
				{/if}
			</fieldset>

			<fieldset class="group" data-testid="section-visitor-read">
				<legend>{$t('sites.bindings.visitorReadTitle')}</legend>
				<Segmented
					label={$t('sites.bindings.visitorReadTitle')}
					width="fill"
					value={draft.visitorRead.scope}
					options={[
						{ value: 'none', label: $t('sites.bindings.scopeNone') },
						{ value: 'own', label: $t('sites.bindings.scopeOwn') }
					]}
					onchange={(value) => setRead('visitorRead', { ...draft.visitorRead, scope: value })}
				/>
				{#if draft.visitorRead.scope !== 'none'}
					{@render readFields('visitorRead', draft.visitorRead)}
				{/if}
				{@render sectionIssues('visitorRead')}
			</fieldset>

			<fieldset class="group" data-testid="section-operator-read">
				<legend>{$t('sites.bindings.operatorReadTitle')}</legend>
				<Segmented
					label={$t('sites.bindings.operatorReadTitle')}
					width="fill"
					value={draft.operatorRead.scope}
					options={[
						{ value: 'none', label: $t('sites.bindings.scopeNone') },
						{ value: 'all', label: $t('sites.bindings.scopeAll') }
					]}
					onchange={(value) => setRead('operatorRead', { ...draft.operatorRead, scope: value })}
				/>
				{#if draft.operatorRead.scope !== 'none'}
					{@render readFields('operatorRead', draft.operatorRead)}
				{/if}
				{@render sectionIssues('operatorRead')}
			</fieldset>

			<fieldset class="group" data-testid="section-update">
				<legend>{$t('sites.bindings.updateTitle')}</legend>
				<Checkbox
					label={$t('sites.bindings.updateOn')}
					checked={draft.update.on}
					onchange={(event) =>
						(draft = { ...draft, update: { ...draft.update, on: event.currentTarget.checked } })}
					data-testid="update-on"
				/>
				{#if draft.update.on}
					{#each fields.filter((field) => field.submit) as field (field.key)}
						<div class="row">
							<Checkbox
								label={field.label}
								checked={draft.update.fields.includes(field.key)}
								onchange={(event) =>
									(draft = {
										...draft,
										update: {
											...draft.update,
											fields: withKey(draft.update.fields, field.key, event.currentTarget.checked)
										}
									})}
							/>
						</div>
						{#each fieldIssues('operatorUpdate', field.key) as text (text)}
							<p class="issue" role="alert">{text}</p>
						{/each}
					{/each}
					{@render blocked(fields.filter((field) => !field.submit && !field.column))}
					{@render sectionIssues('operatorUpdate')}
				{/if}
			</fieldset>
		{/if}

		{#if formError}<TipBar tone="danger" data-testid="binding-error">{formError}</TipBar>{/if}
		{#if issues.some((issue) => issue.section === null)}
			<TipBar tone="danger">{$t('sites.bindings.issue.shape_invalid')}</TipBar>
		{/if}
	</div>
	{#snippet footer()}
		<Button variant="secondary" disabled={busy} onclick={onclose}>{$t('sites.cancel')}</Button>
		<Button
			variant="primary"
			loading={busy}
			onclick={() => void submit()}
			data-testid="save-binding"
		>
			{$t('sites.bindings.save')}
		</Button>
	{/snippet}
</SidePanel>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		padding: var(--rf-space-5);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		margin: 0;
		padding: 0;
		border: 0;
	}
	legend {
		padding: 0;
		margin-block-end: var(--rf-space-3);
		font-weight: 600;
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-5);
	}
	.muted {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.issue {
		margin: 0;
		color: var(--rf-danger);
		font-size: var(--rf-size-secondary);
	}
</style>
