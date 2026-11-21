<script lang="ts">
	import { t } from 'svelte-i18n';
	import Select from './Select.svelte';
	import type { SelectOption } from './listbox';

	let status = $state<string | null>('review');
	let empty = $state<string | null>(null);
	let many = $state<string | null>(null);

	const options = $derived<SelectOption[]>([
		{ value: 'backlog', label: $t('ui.select.sample.backlog'), color: 'cat-1' },
		{ value: 'progress', label: $t('ui.select.sample.progress'), color: 'cat-2' },
		{ value: 'review', label: $t('ui.select.sample.review'), color: 'cat-3' },
		{ value: 'done', label: $t('ui.select.sample.done'), color: 'cat-4' },
		{
			value: 'blocked',
			label: $t('ui.select.sample.blocked'),
			disabled: true,
			disabledReason: $t('ui.select.sample.blockedReason')
		}
	]);
	const long = $derived<SelectOption[]>(
		Array.from({ length: 12 }, (_, i) => ({
			value: `option-${i + 1}`,
			label: `${$t('ui.select.sample.label')} ${i + 1}`
		}))
	);
	const failing = async (): Promise<SelectOption[]> => {
		throw new Error('unavailable');
	};
	const pending = (): Promise<SelectOption[]> => new Promise(() => {});
</script>

<section data-gallery-section="select">
	<h2>select</h2>
	<div class="col">
		<div data-gallery="select:default">
			<Select
				label={$t('ui.select.sample.label')}
				placeholder={$t('ui.select.sample.choose')}
				{options}
				value={status}
				onchange={(v) => (status = v)}
			/>
		</div>
		<div data-gallery="select:empty">
			<Select
				label={$t('ui.select.sample.label')}
				placeholder={$t('ui.select.sample.choose')}
				{options}
				value={empty}
				onchange={(v) => (empty = v)}
				allowEmpty
			/>
		</div>
		<div data-gallery="select:search">
			<Select
				label={$t('ui.select.sample.label')}
				placeholder={$t('ui.select.sample.choose')}
				options={long}
				value={many}
				onchange={(v) => (many = v)}
			/>
		</div>
		<div data-gallery="select:invalid">
			<Select
				label={$t('ui.select.sample.label')}
				placeholder={$t('ui.select.sample.choose')}
				{options}
				value={null}
				onchange={() => {}}
				required
				marking="required"
				error={$t('ui.select.sample.error')}
			/>
		</div>
		<div data-gallery="select:disabled">
			<Select
				label={$t('ui.select.sample.label')}
				{options}
				value="done"
				onchange={() => {}}
				disabled
				disabledReason={$t('ui.select.sample.blockedReason')}
			/>
		</div>
		<div data-gallery="select:readonly">
			<Select
				label={$t('ui.select.sample.label')}
				{options}
				value="done"
				onchange={() => {}}
				readonly
			/>
		</div>
		<div data-gallery="select:compact">
			<Select
				label={$t('ui.select.sample.label')}
				{options}
				value="done"
				onchange={() => {}}
				density="compact"
			/>
		</div>
		<div data-gallery="select:loading">
			<Select
				label={$t('ui.select.sample.label')}
				options={pending}
				value={null}
				onchange={() => {}}
			/>
		</div>
		<div data-gallery="select:failed">
			<Select
				label={$t('ui.select.sample.label')}
				options={failing}
				value={null}
				onchange={() => {}}
			/>
		</div>
	</div>
</section>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 320px;
	}
</style>
