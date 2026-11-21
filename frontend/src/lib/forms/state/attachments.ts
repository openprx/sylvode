import { apiClient } from '#lib/api/client.js';
import { type FormAttachment, type FormField, formsApi } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import { safeImageUrl, safeLinkUrl } from '#lib/utils/safe-url.js';
import type { SignatureDraftScope } from '#lib/forms/page-types.js';
import {
	attachmentContentType,
	attachmentFileName,
	attachmentStorageName,
	isSvgDataUrl,
	recordValueUrl,
	signaturePoint
} from '#lib/forms/attachment-files.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class AttachmentsController extends FormsController {
	attachmentUrl = (field: FormField): string => {
		const value = this.s.recordDraft[field.key];

		if (typeof value === 'string') return value.trim();
		if (isRecord(value) && typeof value.url === 'string') return value.url.trim();

		return '';
	};

	attachmentPreviewUrl = (field: FormField): string => {
		return safeImageUrl(this.attachmentUrl(field));
	};

	uploadFormAttachmentFile = async (
		file: File
	): Promise<{ url: string; filename: string; thumbnail_url?: string } | null> => {
		const formData = new FormData();

		formData.append('file', file);

		const headers = new Headers();
		const token = apiClient.getToken();

		if (token) headers.set('Authorization', `Bearer ${token}`);

		try {
			const response = await fetch('/api/v1/upload', { method: 'POST', headers, body: formData });

			const result = (await response.json()) as {
				code?: number;
				message?: string;
				data?: { url?: string; filename?: string; thumbnail_url?: string };
			};

			if (result.code === 0 && result.data?.url) {
				return {
					url: result.data.url,
					filename: result.data.filename || attachmentStorageName(file.name || 'file'),
					thumbnail_url: result.data.thumbnail_url
				};
			}

			toast.error(result.message || translate('toast.uploadFail'));
		} catch {
			toast.error(translate('toast.uploadFail'));
		}

		return null;
	};

	fieldAttachments = (fieldKey: string): FormAttachment[] => {
		return this.s.recordAttachments.filter(
			(attachment) => attachment.field_key === fieldKey && !attachment.archived_at
		);
	};

	attachmentSignedUrl = (attachment: FormAttachment): string => {
		return safeLinkUrl(this.s.recordAttachmentSignedUrls[attachment.id] || attachment.url);
	};

	loadAttachmentSignedUrls = async (attachments: FormAttachment[]) => {
		const active = attachments.filter((attachment) => !attachment.archived_at && attachment.url);

		if (active.length === 0) {
			this.s.recordAttachmentSignedUrls = {};

			return;
		}

		const entries = await Promise.all(
			active.map(async (attachment) => {
				const response = await formsApi.getAttachmentSignedDownload(attachment.id);

				return response.code === 0 && response.data?.url
					? [attachment.id, response.data.url]
					: null;
			})
		);

		this.s.recordAttachmentSignedUrls = Object.fromEntries(
			entries.filter((entry): entry is [string, string] => Array.isArray(entry))
		);
	};

	loadRecordAttachments = async (recordId: string) => {
		if (!this.s.selectedForm) return;

		const response = await formsApi.listAttachments(this.s.selectedForm.id, {
			record_id: recordId,
			per_page: 100
		});

		if (response.code !== 0) {
			toast.error(response.message);
			this.s.recordAttachments = [];

			return;
		}

		this.s.recordAttachments = response.data?.items ?? [];
		await this.loadAttachmentSignedUrls(this.s.recordAttachments);
	};

	syncRecordAttachmentValue = async (field: FormField, value: string) => {
		if (!this.s.editingRecordId) return;

		const currentRecord =
			this.s.records.find((record) => record.id === this.s.editingRecordId) ??
			this.s.selectedRecord;
		const values = { ...(currentRecord?.values ?? this.s.recordDraft), [field.key]: value };
		const response = await this.fp.session.updateRecordWithVersion(this.s.editingRecordId, {
			values,
			source: { type: 'web', origin: 'attachment' }
		});

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		this.s.selectedRecord = response.data;
		this.s.recordDraft = { ...this.s.recordDraft, [field.key]: value };
	};

	createAttachmentForField = async (field: FormField) => {
		if (!this.s.selectedForm || !this.s.editingRecordId) {
			toast.error(translate('forms.attachmentSaveRecordFirst'));

			return;
		}

		const url = this.attachmentUrl(field);

		if (!url) {
			toast.error(translate('forms.attachmentUrlRequired'));

			return;
		}

		if (!safeLinkUrl(url)) {
			toast.error(translate('forms.attachmentUrlUnsafe'));

			return;
		}

		this.s.attachmentSubmittingField = field.key;

		const fileName = attachmentFileName(url, field);

		const response = await formsApi.createAttachment(this.s.selectedForm.id, {
			field_key: field.key,
			record_id: this.s.editingRecordId,
			file_name: fileName,
			content_type: attachmentContentType(fileName, field),
			byte_size: 0,
			storage_key: `form-records/${this.s.editingRecordId}/${field.key}/${Date.now()}-${fileName}`,
			url
		});

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			this.s.attachmentSubmittingField = null;

			return;
		}

		this.s.recordAttachments = [
			response.data,
			...this.s.recordAttachments.filter((item) => item.id !== response.data?.id)
		];

		await this.loadAttachmentSignedUrls(this.s.recordAttachments);
		await this.syncRecordAttachmentValue(field, response.data.url);
		this.s.attachmentSubmittingField = null;
		toast.success(translate('forms.attachmentLinked'));
	};

	createAttachmentFromLocalFile = async (field: FormField, event: Event) => {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];

		input.value = '';

		if (!file) return;

		if (!this.s.selectedForm || !this.s.editingRecordId) {
			toast.error(translate('forms.attachmentSaveRecordFirst'));

			return;
		}

		this.s.attachmentSubmittingField = field.key;

		const storageName = attachmentStorageName(
			file.name || (field.type === 'image' ? `${field.key}.png` : `${field.key}.bin`)
		);
		const uploaded = await this.uploadFormAttachmentFile(file);

		if (!uploaded) {
			this.s.attachmentSubmittingField = null;

			return;
		}

		const response = await formsApi.createAttachment(this.s.selectedForm.id, {
			field_key: field.key,
			record_id: this.s.editingRecordId,
			file_name: file.name || storageName,
			content_type: file.type || attachmentContentType(storageName, field),
			byte_size: file.size,
			storage_key: `form-records/${this.s.editingRecordId}/${field.key}/server-upload-${Date.now()}-${uploaded.filename}`,
			url: uploaded.url,
			thumbnail_url: uploaded.thumbnail_url
		});

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);
			this.s.attachmentSubmittingField = null;

			return;
		}

		this.s.recordAttachments = [
			response.data,
			...this.s.recordAttachments.filter((item) => item.id !== response.data?.id)
		];

		await this.loadAttachmentSignedUrls(this.s.recordAttachments);
		await this.syncRecordAttachmentValue(field, response.data.url);
		this.s.attachmentSubmittingField = null;
		toast.success(translate('forms.attachmentLinked'));
	};

	removeAttachment = async (field: FormField, attachment: FormAttachment) => {
		this.s.deletingAttachmentId = attachment.id;

		const response = await formsApi.deleteAttachment(attachment.id);

		if (response.code !== 0) {
			toast.error(response.message);
			this.s.deletingAttachmentId = null;

			return;
		}

		this.s.recordAttachments = this.s.recordAttachments.filter((item) => item.id !== attachment.id);

		const remainingSignedUrls = { ...this.s.recordAttachmentSignedUrls };

		delete remainingSignedUrls[attachment.id];
		this.s.recordAttachmentSignedUrls = remainingSignedUrls;

		if (attachment.url?.startsWith('blob:')) {
			URL.revokeObjectURL(attachment.url);
		}

		const currentValue = recordValueUrl(this.s.recordDraft[field.key]);

		if (currentValue === attachment.url) {
			await this.syncRecordAttachmentValue(field, '');
		}

		this.s.deletingAttachmentId = null;
		toast.success(translate('forms.attachmentRemoved'));
	};

	prepareSignatureCanvas = (canvas: HTMLCanvasElement): CanvasRenderingContext2D | null => {
		const rect = canvas.getBoundingClientRect();
		const pixelRatio = window.devicePixelRatio || 1;
		const width = Math.max(320, Math.round(rect.width || 320));
		const height = Math.max(96, Math.round(rect.height || 112));

		if (
			canvas.width !== Math.round(width * pixelRatio) ||
			canvas.height !== Math.round(height * pixelRatio)
		) {
			canvas.width = Math.round(width * pixelRatio);
			canvas.height = Math.round(height * pixelRatio);
			canvas.style.height = `${height}px`;
		}

		const context = canvas.getContext('2d');

		if (!context) return null;

		context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
		context.lineCap = 'round';
		context.lineJoin = 'round';
		context.lineWidth = 2;
		context.strokeStyle = '#0f172a';

		return context;
	};

	/** Stores a signature value; an SVG data URL is refused and `false` is returned. */
	setSignatureValue = (fieldKey: string, scope: SignatureDraftScope, value: string): boolean => {
		if (isSvgDataUrl(value)) return false;

		if (scope === 'child') {
			this.fp.recordDetail.updateChildDraftValue(fieldKey, value);
		} else {
			this.fp.records.setDraftValue(fieldKey, value);
		}

		return true;
	};

	captureSignatureCanvas = (
		canvas: HTMLCanvasElement,
		fieldKey: string,
		scope: SignatureDraftScope
	) => {
		this.setSignatureValue(fieldKey, scope, canvas.toDataURL('image/png'));
	};

	startSignatureStroke = (
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) => {
		if (readOnly || !(event.currentTarget instanceof HTMLCanvasElement)) return;

		const canvas = event.currentTarget;
		const context = this.prepareSignatureCanvas(canvas);

		if (!context) return;

		const point = signaturePoint(event, canvas);

		context.beginPath();
		context.moveTo(point.x, point.y);
		this.s.activeSignatureCanvas = canvas;

		try {
			canvas.setPointerCapture(event.pointerId);
		} catch {
			// Synthetic smoke-test pointer events may not be registered for capture.
		}

		event.preventDefault();
		this.captureSignatureCanvas(canvas, fieldKey, scope);
	};

	moveSignatureStroke = (
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) => {
		if (readOnly || !(event.currentTarget instanceof HTMLCanvasElement)) return;

		const canvas = event.currentTarget;

		if (this.s.activeSignatureCanvas !== canvas) return;

		const context = this.prepareSignatureCanvas(canvas);

		if (!context) return;

		const point = signaturePoint(event, canvas);

		context.lineTo(point.x, point.y);
		context.stroke();
		event.preventDefault();
		this.captureSignatureCanvas(canvas, fieldKey, scope);
	};

	endSignatureStroke = (
		event: PointerEvent,
		fieldKey: string,
		scope: SignatureDraftScope,
		readOnly: boolean
	) => {
		if (!(event.currentTarget instanceof HTMLCanvasElement)) return;

		const canvas = event.currentTarget;

		if (this.s.activeSignatureCanvas !== canvas) return;

		this.s.activeSignatureCanvas = null;

		if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
		if (!readOnly) this.captureSignatureCanvas(canvas, fieldKey, scope);
	};

	clearSignatureCanvas = (fieldKey: string, scope: SignatureDraftScope, readOnly: boolean) => {
		if (readOnly) return;

		const attribute = scope === 'child' ? 'data-child-signature-canvas' : 'data-signature-canvas';

		const canvas = document.querySelector<HTMLCanvasElement>(`canvas[${attribute}="${fieldKey}"]`);

		if (canvas) {
			const context = this.prepareSignatureCanvas(canvas);

			if (context) {
				context.save();
				context.setTransform(1, 0, 0, 1, 0, 0);
				context.clearRect(0, 0, canvas.width, canvas.height);
				context.restore();
			}
		}

		this.setSignatureValue(fieldKey, scope, '');
	};
}
