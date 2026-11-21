<script lang="ts">
	import { t } from 'svelte-i18n';
	import MultiSelect from './MultiSelect.svelte';
	import type { SelectOption } from '../select/listbox';

	let picked = $state<string[]>(['bug', 'urgent', 'docs']);
	let none = $state<string[]>([]);

	const options = $derived<SelectOption[]>([
		{ value: 'bug', label: $t('ui.multiSelect.sample.bug'), color: 'cat-1' },
		{ value: 'urgent', label: $t('ui.multiSelect.sample.urgent'), color: 'cat-2' },
		{ value: 'docs', label: $t('ui.multiSelect.sample.docs'), color: 'cat-3' },
		{ value: 'infra', label: $t('ui.multiSelect.sample.infra'), color: 'cat-4' }
	]);
</script>

<section data-gallery-section="multi-select">
	<h2>multi-select</h2>
	<div class="col">
		<div data-gallery="multi-select:default">
			<MultiSelect
				label={$t('ui.multiSelect.sample.label')}
				placeholder={$t('ui.multiSelect.sample.choose')}
				{options}
				value={none}
				onchange={(v) => (none = v)}
			/>
		</div>
		<div data-gallery="multi-select:chips">
			<MultiSelect
				label={$t('ui.multiSelect.sample.label')}
				{options}
				value={picked}
				onchange={(v) => (picked = v)}
			/>
		</div>
		<div data-gallery="multi-select:disabled">
			<MultiSelect
				label={$t('ui.multiSelect.sample.label')}
				{options}
				value={['bug']}
				onchange={() => {}}
				disabled
				disabledReason={$t('ui.gallery.reason')}
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
