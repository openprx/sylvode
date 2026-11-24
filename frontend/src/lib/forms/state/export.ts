import { describeResult } from '#lib/i18n/describe.js';
import { formsApi } from '#lib/api/forms.js';
import { csvDownloadBlob } from '#lib/utils/csv.js';
import { toast } from '#lib/stores/toast.js';
import type { AttachmentBundleEntry, AttachmentBundleExport } from '#lib/forms/page-types.js';
import { attachmentContentType, attachmentFileName } from '#lib/forms/attachment-files.js';
import { fieldLabel, fieldType } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { attachmentManifestUrls, createStoredZipBlob, safeZipSegment } from '#lib/forms/zip.js';

export class ExportController extends FormsController {
	exportCurrentView = async () => {
		if (!this.s.selectedForm) return;

		this.s.exportingRecords = true;

		const response = await formsApi.exportRecords(this.s.selectedForm.id, {
			view_id: this.s.selectedViewId || undefined,
			columns: this.s.selectedViewId ? undefined : this.s.visibleFields.map((field) => field.key),
			format: 'csv'
		});

		this.s.exportingRecords = false;

		if (response.code !== 0 || !response.data || !response.data.csv) {
			toast.error(describeResult(response, translate('forms.exportFailed')));

			return;
		}

		const blob = csvDownloadBlob(response.data.csv);
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = response.data.file_name;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		toast.success(translate('forms.recordsExported'));
	};

	exportCurrentViewJson = async () => {
		if (!this.s.selectedForm) return;

		this.s.exportingRecords = true;

		const response = await formsApi.exportRecords(this.s.selectedForm.id, {
			view_id: this.s.selectedViewId || undefined,
			columns: this.s.selectedViewId ? undefined : this.s.visibleFields.map((field) => field.key),
			format: 'json'
		});

		this.s.exportingRecords = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response, translate('forms.exportFailed')));

			return;
		}

		const blob = new Blob([JSON.stringify(response.data, null, 2)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = response.data.file_name;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedCurrentViewJsonRecordCount = response.data.rows.length;
		toast.success(translate('forms.recordsJsonExported'));
	};

	exportAttachmentManifest = () => {
		if (!this.s.selectedForm) return;

		const attachmentFields = this.s.fields.filter((field) =>
			['attachment', 'image'].includes(fieldType(field))
		);

		const attachments = this.s.viewRecords.flatMap((record) =>
			attachmentFields.flatMap((field) => {
				const value = record.values?.[field.key];
				const urls = attachmentManifestUrls(value);

				return urls.map((url) => ({
					record_id: record.id,
					record_title: record.title,
					field_key: field.key,
					field_label: fieldLabel(field),
					field_type: fieldType(field),
					url,
					file_name: attachmentFileName(url, field),
					content_type: attachmentContentType(attachmentFileName(url, field), field)
				}));
			})
		);

		const manifest = {
			form_id: this.s.selectedForm.id,
			form_key: this.s.selectedForm.key,
			view_id: this.s.selectedViewId || null,
			view_name: this.s.selectedView?.name ?? null,
			exported_at: new Date().toISOString(),
			record_count: this.s.viewRecords.length,
			attachment_count: attachments.length,
			attachments
		};

		const blob = new Blob([JSON.stringify(manifest, null, 2)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-attachment-manifest.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedAttachmentManifestCount = attachments.length;
		toast.success(translate('forms.attachmentManifestExported'));
	};

	collectAttachmentBundle = async (): Promise<AttachmentBundleExport | null> => {
		if (!this.s.selectedForm) return null;

		const attachmentFields = this.s.fields.filter((field) =>
			['attachment', 'image'].includes(fieldType(field))
		);
		const recordIds = new Set(this.s.viewRecords.map((record) => record.id));

		this.s.exportingRecords = true;

		const metadataResponse = await formsApi.listAttachments(this.s.selectedForm.id, {
			per_page: 200
		});

		this.s.exportingRecords = false;

		if (metadataResponse.code !== 0 || !metadataResponse.data) {
			toast.error(describeResult(metadataResponse, translate('forms.exportFailed')));

			return null;
		}

		const metadataAttachments = metadataResponse.data.items.filter(
			(attachment) =>
				!attachment.archived_at &&
				attachment.record_id &&
				recordIds.has(attachment.record_id) &&
				attachmentFields.some((field) => field.key === attachment.field_key)
		);

		const signedEntries = await Promise.all(
			metadataAttachments.map(async (attachment) => {
				const response = await formsApi.getAttachmentSignedDownload(attachment.id);

				return [
					attachment.id,
					response.code === 0 && response.data?.url ? response.data.url : attachment.url
				] as const;
			})
		);

		const signedUrls = Object.fromEntries(signedEntries);

		const metadataByValue = new Map(
			metadataAttachments.map((attachment) => [
				`${attachment.record_id ?? ''}:${attachment.field_key}:${attachment.url}`,
				attachment
			])
		);

		const bundledKeys = new Set<string>();

		const attachments: AttachmentBundleEntry[] = this.s.viewRecords.flatMap((record) =>
			attachmentFields.flatMap((field) => {
				const value = record.values?.[field.key];
				const urls = attachmentManifestUrls(value);

				return urls.map((url) => {
					const metadata = metadataByValue.get(`${record.id}:${field.key}:${url}`);
					const bundleKey = `${record.id}:${field.key}:${url}`;

					bundledKeys.add(bundleKey);

					return {
						record_id: record.id,
						record_title: record.title,
						field_key: field.key,
						field_label: fieldLabel(field),
						field_type: fieldType(field),
						url,
						download_url: metadata ? signedUrls[metadata.id] || metadata.url : url,
						thumbnail_url: metadata?.thumbnail_url ?? null,
						file_name: metadata?.file_name ?? attachmentFileName(url, field),
						content_type:
							metadata?.content_type ||
							attachmentContentType(metadata?.file_name ?? attachmentFileName(url, field), field),
						byte_size: metadata?.byte_size ?? 0,
						storage_key: metadata?.storage_key ?? null,
						attachment_id: metadata?.id ?? null,
						source: metadata ? 'metadata' : 'record_value'
					};
				});
			})
		);

		for (const attachment of metadataAttachments) {
			const bundleKey = `${attachment.record_id ?? ''}:${attachment.field_key}:${attachment.url}`;

			if (bundledKeys.has(bundleKey)) continue;

			const record = this.s.viewRecords.find((item) => item.id === attachment.record_id);
			const field = attachmentFields.find((item) => item.key === attachment.field_key);

			attachments.push({
				record_id: attachment.record_id ?? '',
				record_title: record?.title ?? '',
				field_key: attachment.field_key,
				field_label: field ? fieldLabel(field) : attachment.field_key,
				field_type: field ? fieldType(field) : 'attachment',
				url: attachment.url,
				download_url: signedUrls[attachment.id] || attachment.url,
				thumbnail_url: attachment.thumbnail_url ?? null,
				file_name: attachment.file_name,
				content_type: attachment.content_type,
				byte_size: attachment.byte_size,
				storage_key: attachment.storage_key,
				attachment_id: attachment.id,
				source: 'metadata'
			});
		}

		return {
			form_id: this.s.selectedForm.id,
			form_key: this.s.selectedForm.key,
			view_id: this.s.selectedViewId || null,
			view_name: this.s.selectedView?.name ?? null,
			exported_at: new Date().toISOString(),
			record_count: this.s.viewRecords.length,
			attachment_count: attachments.length,
			bundle_format: 'attachment-bundle-json-v1',
			attachments
		};
	};

	exportAttachmentBundle = async () => {
		const bundle = await this.collectAttachmentBundle();

		if (!bundle) return;

		const blob = new Blob([JSON.stringify(bundle, null, 2)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${bundle.form_key}-attachment-bundle.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedAttachmentBundleCount = bundle.attachments.length;
		toast.success(translate('forms.attachmentBundleExported'));
	};

	exportAttachmentPackage = async () => {
		if (!this.s.selectedForm) return;

		const bundle = await this.collectAttachmentBundle();

		if (!bundle) return;

		const packagedFiles = await Promise.all(
			bundle.attachments.map(async (attachment, index) => {
				const baseName = `${String(index + 1).padStart(3, '0')}-${safeZipSegment(attachment.file_name)}`;
				const bytes = await this.fetchAttachmentPackageBytes(attachment);

				if (bytes) {
					return {
						file: { path: `files/${baseName}`, data: bytes },
						manifest: {
							...attachment,
							package_path: `files/${baseName}`,
							package_status: 'included'
						}
					};
				}

				return {
					file: {
						path: `links/${baseName}.url.txt`,
						data: `${attachment.download_url || attachment.url}\n`
					},
					manifest: {
						...attachment,
						package_path: `links/${baseName}.url.txt`,
						package_status: 'external_url'
					}
				};
			})
		);

		const packageManifest = {
			...bundle,
			bundle_format: 'attachment-zip-package-v1',
			file_count: packagedFiles.length,
			binary_file_count: packagedFiles.filter(
				(entry) => entry.manifest.package_status === 'included'
			).length,
			attachments: packagedFiles.map((entry) => entry.manifest)
		};

		const zip = createStoredZipBlob([
			{
				path: 'manifest.json',
				data: JSON.stringify(packageManifest, null, 2)
			},
			...packagedFiles.map((entry) => entry.file)
		]);

		const url = URL.createObjectURL(zip);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-attachment-package.zip`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedAttachmentPackageCount = bundle.attachments.length;
		this.s.exportedAttachmentPackageFileCount = packagedFiles.length;
		toast.success(translate('forms.attachmentPackageExported'));
	};

	fetchAttachmentPackageBytes = async (
		attachment: AttachmentBundleEntry
	): Promise<Uint8Array | null> => {
		const directUrl = this.sameOriginUrl(attachment.url);
		const signedUrl = this.sameOriginUrl(attachment.download_url);

		for (const candidate of [directUrl, signedUrl].filter(Boolean)) {
			try {
				const response = await fetch(candidate, { redirect: 'manual' });

				if (!response.ok) continue;

				return new Uint8Array(await response.arrayBuffer());
			} catch {
				continue;
			}
		}

		return null;
	};

	sameOriginUrl = (value: string | null | undefined): string => {
		if (!value) return '';

		try {
			const url = new URL(value, window.location.origin);

			return url.origin === window.location.origin ? url.toString() : '';
		} catch {
			return '';
		}
	};
}
