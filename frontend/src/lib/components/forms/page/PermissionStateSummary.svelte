<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Shield } from '@lucide/svelte';
	import { permissionFieldList } from '#lib/forms/permission-drafts.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
</script>

<section
	class="rounded-card border border-line bg-raised px-3 py-2 text-sm"
	data-permission-state-summary
	data-permission-state-mode={s.activeMode}
	data-permission-state-scope={s.memberRecordScopeDraft}
	data-permission-state-hidden-fields={s.memberHiddenFieldLabels.join('|')}
	data-permission-state-locked-fields={s.memberLockedFieldLabels.join('|')}
>
	<div class="flex flex-wrap items-center gap-2 text-xs">
		<span class="inline-flex items-center gap-1 font-medium text-fg">
			<Shield class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			{$t('forms.permissionState')}
		</span>

		<span class="rounded-control border border-line bg-nav px-2 py-1 text-fg"
			>{$t('forms.recordScope')}: {$t(
				`forms.recordScopes.${s.memberRecordScopeDraft}`
			)}</span
		>

		<span
			class="rounded-control border border-line bg-warning-soft px-2 py-1 text-warning"
			>{$t('forms.hiddenFieldEffect')}: {permissionFieldList(
				s.memberHiddenFieldLabels
			)}</span
		>

		<span class="rounded-control border border-line bg-nav px-2 py-1 text-fg"
			>{$t('forms.lockedFieldEffect')}: {permissionFieldList(
				s.memberLockedFieldLabels
			)}</span
		>
	</div>
</section>
