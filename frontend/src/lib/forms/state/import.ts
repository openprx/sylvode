import { type FormField, type FormRecordImportRow, formsApi } from '#lib/api/forms.js';
import { csvDownloadBlob } from '#lib/utils/csv.js';
import { toast } from '#lib/stores/toast.js';
import type { ImportMappingTransform } from '#lib/forms/page-types.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import {
	applyImportMappingTransform,
	csvCell,
	importCellValue,
	isValidImportMappingTransform,
	normalizeImportItem,
	parseCsvLines,
	sampleImportValue
} from '#lib/forms/import-csv.js';
import { isRecord } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class ImportController extends FormsController {
	openImportRecords = () => {
		this.s.importText = this.sampleImportText();
		this.s.importRows = [];
		this.s.importPreview = null;
		this.s.importWizardStep = 'source';
		this.s.exportedImportTemplate = false;
		this.s.importedImportFileName = '';
		this.s.importedImportFileLoaded = false;
		this.refreshImportMappingFromText();
		this.s.showImportRecords = true;
	};

	closeImportRecords = () => {
		this.s.showImportRecords = false;
		this.s.importPreview = null;
		this.s.importWizardStep = 'source';
	};

	resetImportPreviewForSourceChange = () => {
		this.s.importRows = [];
		this.s.importPreview = null;
		this.s.importWizardStep = 'source';
	};

	continueImportFromSource = () => {
		this.refreshImportMappingFromText();
		this.s.importPreview = null;

		if (this.s.importMappingHeaders.length > 0) {
			this.s.importWizardStep = 'mapping';

			return;
		}

		void this.previewImportRecords();
	};

	continueImportFromMapping = () => {
		if (this.s.importMappingHeaders.length > 0 && !this.s.importMappingApplied) {
			this.applyImportMapping();
		}

		void this.previewImportRecords();
	};

	sampleImportText = (): string => {
		if (this.s.visibleFields.length === 0) return '[]';

		return this.importTemplateCsv();
	};

	importTemplateCsv = (): string => {
		const header = ['Title', ...this.s.visibleFields.map((field) => field.key)]
			.map(csvCell)
			.join(',');

		const values = ['', ...this.s.visibleFields.map((field) => sampleImportValue(field))]
			.map(csvCell)
			.join(',');

		return `${header}\n${values}\n`;
	};

	exportImportTemplate = () => {
		if (!this.s.selectedForm) return;

		const csv = this.importTemplateCsv();
		const blob = csvDownloadBlob(csv);
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-import-template.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedImportTemplate = true;
		toast.success(translate('forms.importTemplateExported'));
	};

	loadImportFile = async (event: Event) => {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];

		if (!file) return;

		try {
			this.s.importText = await file.text();
			this.resetImportPreviewForSourceChange();
			this.s.importedImportFileName = file.name;
			this.s.importedImportFileLoaded = true;
			this.refreshImportMappingFromText();
			toast.success(translate('forms.importFileLoaded', { values: { name: file.name } }));
		} catch {
			toast.error(translate('forms.importFileFailed'));
		} finally {
			input.value = '';
		}
	};

	refreshImportMappingFromText = () => {
		const trimmed = this.s.importText.trim();

		if (!trimmed || trimmed.startsWith('[')) {
			this.resetImportMappingState();

			return;
		}

		const headers = parseCsvLines(trimmed)[0]?.map((header) => header.trim()) ?? [];
		const savedTemplate = this.readImportMappingTemplate(headers);

		this.s.importMappingHeaders = headers;
		this.s.importMappingSelections =
			savedTemplate?.selections ??
			headers.map((header) => this.defaultImportMappingSelection(header));
		this.s.importMappingTransforms = savedTemplate?.transforms ?? headers.map(() => 'trim');
		this.s.importMappingApplied = false;
		this.s.importMappingTemplateAvailable = Boolean(savedTemplate);
		this.s.importMappingTemplateSaved = false;
		this.s.importMappingTemplateRestored = Boolean(savedTemplate);
	};

	resetImportMappingState = () => {
		this.s.importMappingHeaders = [];
		this.s.importMappingSelections = [];
		this.s.importMappingTransforms = [];
		this.s.importMappingApplied = false;
		this.s.importMappingTemplateAvailable = false;
		this.s.importMappingTemplateSaved = false;
		this.s.importMappingTemplateRestored = false;
	};

	defaultImportMappingSelection = (header: string): string => {
		if (header.toLowerCase() === 'title' || header === '标题') return '__title';

		return this.fieldForImportHeader(header)?.key ?? '__skip';
	};

	importMappingTemplateKey = (headers = this.s.importMappingHeaders): string => {
		if (!this.s.selectedForm || headers.length === 0) return '';

		return `openpr:forms:import-mapping:${this.s.selectedForm.id}:${headers.join('\u001f')}`;
	};

	isValidImportMappingSelection = (selection: string): boolean => {
		return (
			selection === '__skip' ||
			selection === '__title' ||
			this.s.fields.some((field) => field.key === selection)
		);
	};

	readImportMappingTemplate = (
		headers = this.s.importMappingHeaders
	): { selections: string[]; transforms: ImportMappingTransform[] } | null => {
		if (typeof window === 'undefined') return null;

		const key = this.importMappingTemplateKey(headers);

		if (!key) return null;

		try {
			const raw = window.localStorage.getItem(key);

			if (!raw) return null;

			const parsed = JSON.parse(raw) as unknown;

			if (!isRecord(parsed) || !Array.isArray(parsed.selections)) return null;

			const selections = parsed.selections.filter(
				(selection): selection is string => typeof selection === 'string'
			);

			if (
				selections.length !== headers.length ||
				!selections.every(this.isValidImportMappingSelection)
			)
				return null;

			const transforms = Array.isArray(parsed.transforms)
				? parsed.transforms.filter(
						(transform): transform is ImportMappingTransform =>
							typeof transform === 'string' && isValidImportMappingTransform(transform)
					)
				: headers.map(() => 'trim' as ImportMappingTransform);

			if (transforms.length !== headers.length) return null;

			return { selections, transforms };
		} catch {
			return null;
		}
	};

	saveImportMappingTemplate = () => {
		if (typeof window === 'undefined') return;

		const key = this.importMappingTemplateKey();

		if (!key || this.s.importMappingHeaders.length === 0) {
			toast.error(translate('forms.importMappingTemplateUnavailable'));

			return;
		}

		const selections = this.s.importMappingSelections.map((selection) =>
			this.isValidImportMappingSelection(selection) ? selection : '__skip'
		);
		const transforms = this.s.importMappingTransforms.map((transform) =>
			isValidImportMappingTransform(transform) ? transform : 'trim'
		);

		window.localStorage.setItem(
			key,
			JSON.stringify({
				form_id: this.s.selectedForm?.id,
				headers: this.s.importMappingHeaders,
				selections,
				transforms,
				updated_at: new Date().toISOString()
			})
		);

		this.s.importMappingSelections = selections;
		this.s.importMappingTransforms = transforms;
		this.s.importMappingTemplateAvailable = true;
		this.s.importMappingTemplateSaved = true;
		this.s.importMappingTemplateRestored = false;
		toast.success(translate('forms.importMappingTemplateSaved'));
	};

	restoreImportMappingTemplate = () => {
		const savedTemplate = this.readImportMappingTemplate();

		if (!savedTemplate) {
			toast.error(translate('forms.importMappingTemplateUnavailable'));

			return;
		}

		this.s.importMappingSelections = savedTemplate.selections;
		this.s.importMappingTransforms = savedTemplate.transforms;
		this.s.importMappingApplied = false;
		this.s.importMappingTemplateAvailable = true;
		this.s.importMappingTemplateRestored = true;
		toast.success(translate('forms.importMappingTemplateRestored'));
	};

	applyImportMapping = () => {
		const lines = parseCsvLines(this.s.importText.trim());

		if (lines.length < 2 || this.s.importMappingHeaders.length === 0) {
			toast.error(translate('forms.importNeedsRows'));

			return;
		}

		const selectedColumns = this.s.importMappingSelections
			.map((selection, index) => ({
				selection,
				index,
				transform: this.s.importMappingTransforms[index] ?? 'trim'
			}))
			.filter((column) => column.selection !== '__skip');

		if (selectedColumns.length === 0) {
			toast.error(translate('forms.importMappingNeedsField'));

			return;
		}

		const remappedRows = [
			selectedColumns.map((column) =>
				column.selection === '__title' ? 'Title' : column.selection
			),
			...lines
				.slice(1)
				.map((row) =>
					selectedColumns.map((column) =>
						applyImportMappingTransform(row[column.index] ?? '', column.transform)
					)
				)
		];

		this.s.importText = remappedRows.map((row) => row.map(csvCell).join(',')).join('\n');
		this.s.importRows = [];
		this.s.importPreview = null;
		this.s.importMappingApplied = true;
		this.s.importWizardStep = 'mapping';
		toast.success(translate('forms.importMappingApplied'));
	};

	parseImportRowsFromText = (): FormRecordImportRow[] => {
		const trimmed = this.s.importText.trim();

		if (!trimmed) throw new Error(translate('forms.importEmpty'));

		if (trimmed.startsWith('[')) {
			const parsed = JSON.parse(trimmed) as unknown;

			if (!Array.isArray(parsed)) throw new Error(translate('forms.importInvalidJson'));

			return parsed.map((item, index) => normalizeImportItem(item, index + 1));
		}

		return this.parseCsvImport(trimmed);
	};

	parseCsvImport = (text: string): FormRecordImportRow[] => {
		const lines = parseCsvLines(text);

		if (lines.length < 2) throw new Error(translate('forms.importNeedsRows'));

		const headers = lines[0].map((header) => header.trim());
		const rows: FormRecordImportRow[] = [];

		for (let index = 1; index < lines.length; index += 1) {
			const cells = lines[index];

			if (cells.every((cell) => cell.trim() === '')) continue;

			const values: Record<string, unknown> = {};
			let title: string | undefined;

			for (let column = 0; column < headers.length; column += 1) {
				const header = headers[column] ?? '';
				const raw = cells[column] ?? '';

				if (header.toLowerCase() === 'title' || header === '标题') {
					title = raw.trim() || undefined;

					continue;
				}

				const field = this.fieldForImportHeader(header);

				if (!field) continue;

				const value = importCellValue(field, raw);

				if (value !== undefined) values[field.key] = value;
			}

			rows.push({
				row_number: index,
				title,
				values,
				source: { type: 'web', origin: 'import' }
			});
		}

		return rows;
	};

	fieldForImportHeader = (header: string): FormField | null => {
		const normalized = header.trim();

		if (!normalized) return null;

		return (
			this.s.fields.find(
				(field) =>
					field.key === normalized ||
					fieldLabel(field) === normalized ||
					field.key.toLowerCase() === normalized.toLowerCase()
			) ?? null
		);
	};

	previewImportRecords = async () => {
		if (!this.s.selectedForm) return;

		let rows: FormRecordImportRow[];

		try {
			rows = this.parseImportRowsFromText();
		} catch (error) {
			toast.error(error instanceof Error ? error.message : translate('forms.importFailed'));

			return;
		}

		this.s.importPreviewing = true;

		const response = await formsApi.previewImportRecords(this.s.selectedForm.id, rows);

		this.s.importPreviewing = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message || translate('forms.importFailed'));

			return;
		}

		this.s.importRows = rows;
		this.s.importPreview = response.data;
		this.s.importWizardStep = 'preview';
		toast.success(translate('forms.importPreviewReady'));
	};

	commitImportRecords = async () => {
		if (!this.s.selectedForm || this.s.importRows.length === 0) return;

		this.s.importingRecords = true;

		const response = await formsApi.importRecords(this.s.selectedForm.id, this.s.importRows);

		this.s.importingRecords = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message || translate('forms.importFailed'));

			return;
		}

		this.s.importPreview = response.data;

		if (response.data.invalid_rows > 0) {
			this.s.importWizardStep = 'preview';
			toast.error(translate('forms.importHasInvalidRows'));

			return;
		}

		this.s.importWizardStep = 'commit';
		this.s.showImportRecords = false;
		await this.fp.loaders.loadRecords();
		toast.success(
			translate('forms.recordsImported', { values: { count: response.data.created_count } })
		);
	};
}
