<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { AlertTriangle, Shield } from '@lucide/svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import { formPermissionActions } from '#lib/forms/page-constants.js';
	import type { FormRecordScope } from '#lib/forms/page-types.js';
	import { permissionActionLabel, permissionFieldList } from '#lib/forms/permission-drafts.js';
	import { selectValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		saveMemberPermissions,
		setMemberPermission,
		setMemberRecordScope,
		setMemberFieldRead,
		setMemberFieldWrite
	} = fp.permissions;
</script>

<section class="rounded-card border border-line bg-raised p-4" data-form-mode="permissions">
	<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<div class="flex items-center gap-2">
				<Shield class="h-4 w-4 text-fg-muted" aria-hidden="true" />

				<h3 class="text-base font-semibold text-fg">
					{$t('forms.permissions')}
				</h3>
			</div>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.permissionsDescription')}
			</p>
		</div>

		<Button
			variant="primary"
			size="sm"
			loading={s.permissionsSaving}
			onclick={saveMemberPermissions}>{$t('forms.savePermissions')}</Button
		>
	</div>

	{#if s.permissionsLoading}
		<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
	{:else}
		<div class="grid gap-3 lg:grid-cols-[180px_minmax(0,1fr)]">
			<div class="rounded-card border border-line bg-nav p-3 text-sm">
				<p class="font-medium text-fg">{$t('roles.member')}</p>

				<p class="mt-1 text-xs text-fg-muted">
					{$t('forms.memberPermissionHint')}
				</p>
			</div>

			<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
				{#each formPermissionActions as action, index (index)}
					<label
						class="flex min-h-12 items-center gap-2 rounded-control border border-line px-3 py-2 text-sm text-fg"
					>
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-input accent-primary"
							checked={s.memberPermissionDraft[action]}
							onchange={(event) =>
								setMemberPermission(
									action,
									event.currentTarget instanceof HTMLInputElement
										? event.currentTarget.checked
										: false
								)}
						/>

						<span>{permissionActionLabel(action)}</span>
					</label>
				{/each}
			</div>

			<label class="rounded-control border border-line px-3 py-2 text-sm text-fg">
				<span class="block text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.recordScope')}</span
				>

				<select
					value={s.memberRecordScopeDraft}
					onchange={(event) => setMemberRecordScope(selectValue(event) as FormRecordScope)}
					class="mt-2 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('forms.recordScopeAll')}</option>
					<option value="owned">{$t('forms.recordScopeOwned')}</option>
				</select>

				<span class="mt-1 block text-xs text-fg-muted">{$t('forms.recordScopeHint')}</span>
			</label>

			<div class="lg:col-span-2 rounded-card border border-line px-3 py-2 text-sm">
				<div class="flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
					<div>
						<p class="text-xs font-medium uppercase text-fg-muted">
							{$t('forms.fieldPermissions')}
						</p>

						<p class="text-xs text-fg-muted">
							{$t('forms.fieldPermissionsHint')}
						</p>
					</div>

					<div class="flex gap-3 text-xs text-fg-muted">
						<span
							data-field-read-denied-count={s.fields.filter(
								(field) => s.memberFieldReadDraft[field.key] === false
							).length}
						>
							{$t('forms.fieldReadDeniedCount', {
								values: {
									count: s.fields.filter((field) => s.memberFieldReadDraft[field.key] === false)
										.length
								}
							})}
						</span>

						<span
							data-field-write-denied-count={s.fields.filter(
								(field) => s.memberFieldWriteDraft[field.key] === false
							).length}
						>
							{$t('forms.fieldWriteDeniedCount', {
								values: {
									count: s.fields.filter((field) => s.memberFieldWriteDraft[field.key] === false)
										.length
								}
							})}
						</span>
					</div>
				</div>

				<div class="mt-3 grid gap-2 md:grid-cols-2" data-permission-effect-summary>
					<div
						class="rounded-card border border-line bg-warning-soft px-3 py-2"
						data-permission-effect-hidden-fields={s.memberHiddenFieldLabels.join('|')}
					>
						<div class="flex items-center gap-2">
							<AlertTriangle class="h-4 w-4 text-warning" aria-hidden="true" />

							<p class="text-xs font-medium uppercase text-warning">
								{$t('forms.hiddenFieldEffect')}
							</p>
						</div>

						<p class="mt-1 text-xs text-warning">
							{permissionFieldList(s.memberHiddenFieldLabels)}
						</p>

						<p class="mt-1 text-xs text-warning">
							{$t('forms.hiddenFieldEffectHint')}
						</p>
					</div>

					<div
						class="rounded-card border border-line bg-nav px-3 py-2"
						data-permission-effect-locked-fields={s.memberLockedFieldLabels.join('|')}
					>
						<div class="flex items-center gap-2">
							<Shield class="h-4 w-4 text-fg-muted" aria-hidden="true" />

							<p class="text-xs font-medium uppercase text-fg-muted">
								{$t('forms.lockedFieldEffect')}
							</p>
						</div>

						<p class="mt-1 text-xs text-fg">
							{permissionFieldList(s.memberLockedFieldLabels)}
						</p>

						<p class="mt-1 text-xs text-fg-muted">
							{$t('forms.lockedFieldEffectHint')}
						</p>
					</div>
				</div>

				<div class="mt-3 grid gap-2">
					{#each s.fields as field (field.key)}
						<div
							class="grid min-h-10 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 rounded-card border border-line px-3 py-2 text-sm text-fg"
						>
							<span class="min-w-0 truncate">{field.label ?? field.key}</span>

							<label class="flex items-center gap-2 text-xs">
								<input
									type="checkbox"
									class="h-4 w-4 rounded border-input accent-primary"
									checked={s.memberFieldReadDraft[field.key] ?? true}
									data-field-read-permission={field.key}
									onchange={(event) =>
										setMemberFieldRead(
											field.key,
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.checked
												: false
										)}
								/>

								<span>{$t('forms.fieldRead')}</span>
							</label>

							<label class="flex items-center gap-2 text-xs">
								<input
									type="checkbox"
									class="h-4 w-4 rounded border-input accent-primary"
									checked={s.memberFieldWriteDraft[field.key] ?? true}
									data-field-write-permission={field.key}
									onchange={(event) =>
										setMemberFieldWrite(
											field.key,
											event.currentTarget instanceof HTMLInputElement
												? event.currentTarget.checked
												: false
										)}
								/>

								<span>{$t('forms.fieldWrite')}</span>
							</label>
						</div>
					{/each}
				</div>
			</div>
		</div>

		{#if s.formPermissions}
			<div class="mt-3 rounded-control bg-nav px-3 py-2 text-xs text-fg-muted">
				{$t('forms.currentEffectiveRole', {
					values: { role: s.formPermissions.effective.role }
				})} · {formPermissionActions
					.filter((action) => s.formPermissions?.effective.actions[action])
					.map(permissionActionLabel)
					.join(' / ')} · {$t(`forms.recordScopes.${s.formPermissions.effective.record_scope}`)}
			</div>
		{/if}
	{/if}
</section>
