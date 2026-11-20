<script lang="ts">
	import { t } from 'svelte-i18n';
	import SettingRow from './SettingRow.svelte';
	import StatusChip from '../status-chip/StatusChip.svelte';
	import Button from '../button/Button.svelte';
</script>

<section data-gallery-section="setting-row">
	<h2>setting-row</h2>
	<div class="column">
		<span data-gallery="setting-row:value">
			<SettingRow
				label={$t('ui.settingRow.sample.label')}
				value={$t('ui.settingRow.sample.value')}
			/>
		</span>
		<span data-gallery="setting-row:chip-action">
			<SettingRow label={$t('ui.settingRow.sample.label')} value={$t('ui.settingRow.sample.value')}>
				{#snippet chip()}
					<StatusChip tone="success">{$t('ui.statusChip.success')}</StatusChip>
				{/snippet}
				{#snippet action()}
					<Button variant="text" size="sm">{$t('ui.settingRow.sample.change')}</Button>
				{/snippet}
			</SettingRow>
		</span>
	</div>
</section>

<style>
	.column {
		display: flex;
		flex-direction: column;
		max-width: 560px;
	}
</style>
