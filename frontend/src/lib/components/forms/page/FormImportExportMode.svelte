<svelte:options runes />

<script lang="ts">
	import { FileDown, FileText, FileUp } from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		exportCurrentView,
		exportCurrentViewJson,
		exportAttachmentManifest,
		exportAttachmentBundle,
		exportAttachmentPackage
	} = fp.exporter;
	const { openImportRecords, exportImportTemplate } = fp.importer;
</script>

<section class="rounded-card border border-line bg-raised p-4" data-form-mode="import-export">
	<div class="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<div class="flex items-center gap-2">
				<FileUp class="h-4 w-4 text-fg-muted" aria-hidden="true" />

				<h3 class="text-base font-semibold text-fg">
					{$t('forms.modes.importExport')}
				</h3>
			</div>
		</div>

		<Button variant="primary" size="sm" onclick={openImportRecords}>
			<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.importRecords')}
		</Button>
	</div>

	<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface"
			onclick={exportImportTemplate}
			disabled={!s.selectedForm || s.visibleFields.length === 0}
			data-import-template-export
			data-import-template-exported={s.exportedImportTemplate ? 'true' : undefined}
		>
			<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.downloadImportTemplate')}</span>
		</button>

		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={exportCurrentView}
			disabled={s.exportingRecords}
		>
			<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.exportCurrentView')}</span>
		</button>

		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={exportCurrentViewJson}
			disabled={s.exportingRecords}
			data-current-view-export-json
			data-current-view-json-record-count={s.exportedCurrentViewJsonRecordCount ?? ''}
		>
			<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.exportCurrentViewJson')}</span>
		</button>

		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={exportAttachmentManifest}
			disabled={s.exportingRecords}
			data-attachment-manifest-export
			data-attachment-manifest-count={s.exportedAttachmentManifestCount ?? ''}
		>
			<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.exportAttachmentManifest')}</span>
		</button>

		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={() => void exportAttachmentBundle()}
			disabled={s.exportingRecords}
			data-attachment-bundle-export
			data-attachment-bundle-count={s.exportedAttachmentBundleCount ?? ''}
		>
			<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.exportAttachmentBundle')}</span>
		</button>

		<button
			type="button"
			class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={() => void exportAttachmentPackage()}
			disabled={s.exportingRecords}
			data-attachment-package-export
			data-attachment-package-count={s.exportedAttachmentPackageCount ?? ''}
			data-attachment-package-file-count={s.exportedAttachmentPackageFileCount ?? ''}
		>
			<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<span class="font-medium text-fg">{$t('forms.exportAttachmentPackage')}</span>
		</button>
	</div>
</section>
