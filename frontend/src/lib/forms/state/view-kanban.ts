import type { FormField, FormRecord } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';
import { groupRecordsByField } from '#lib/forms/record-grouping.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { viewConfigString } from '#lib/forms/view-config.js';

export class ViewKanbanController extends FormsController {
	updateKanbanRecordField = async (record: FormRecord, field: FormField, value: string) => {
		if (!value || record.values[field.key] === value) return;

		if (!this.kanbanMoveAllowed(record, value)) {
			toast.error(translate('forms.kanbanWipLimitReached'));

			return;
		}

		this.s.updatingKanbanRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'kanban' }
		});

		this.s.updatingKanbanRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.kanbanStatusUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	selectedKanbanRecords = (): FormRecord[] => {
		const selectedIds = new Set(this.s.selectedKanbanRecordIds);

		return this.s.records.filter((record) => selectedIds.has(record.id));
	};

	isKanbanRecordSelected = (recordId: string): boolean => {
		return this.s.selectedKanbanRecordIds.includes(recordId);
	};

	setKanbanRecordSelected = (recordId: string, selected: boolean) => {
		this.s.selectedKanbanRecordIds = selected
			? Array.from(new Set([...this.s.selectedKanbanRecordIds, recordId]))
			: this.s.selectedKanbanRecordIds.filter((id) => id !== recordId);
	};

	clearKanbanSelection = () => {
		this.s.selectedKanbanRecordIds = [];
	};

	kanbanBulkMoveAllowed = (selectedRecords: FormRecord[], targetValue: string): boolean => {
		if (!this.s.kanbanGroupField || selectedRecords.length === 0) return false;

		const limit = this.kanbanWipLimit(targetValue);

		if (limit === null) return true;

		const selectedIds = new Set(selectedRecords.map((record) => record.id));
		const kanbanGroupField = this.s.kanbanGroupField;
		const currentTargetCount = this.s.records.filter(
			(record) =>
				!selectedIds.has(record.id) &&
				String(record.values[kanbanGroupField.key] ?? '') === targetValue
		).length;

		return currentTargetCount + selectedRecords.length <= limit;
	};

	bulkMoveKanbanRecords = async () => {
		if (!this.s.kanbanGroupField || !this.s.bulkKanbanTarget) return;

		const selectedRecords = this.selectedKanbanRecords();

		if (selectedRecords.length === 0) {
			toast.error(translate('forms.kanbanBulkSelectFirst'));

			return;
		}

		if (!this.kanbanBulkMoveAllowed(selectedRecords, this.s.bulkKanbanTarget)) {
			toast.error(translate('forms.kanbanWipLimitReached'));

			return;
		}

		this.s.bulkMovingKanban = true;

		const updatedRecords: FormRecord[] = [];

		for (const record of selectedRecords) {
			if (String(record.values[this.s.kanbanGroupField.key] ?? '') === this.s.bulkKanbanTarget)
				continue;

			const response = await this.fp.session.updateRecordWithVersion(record.id, {
				values: { ...record.values, [this.s.kanbanGroupField.key]: this.s.bulkKanbanTarget },
				source: { type: 'web', origin: 'kanban_bulk' }
			});

			if (response.code !== 0 || !response.data) {
				this.s.bulkMovingKanban = false;
				toast.error(response.message);

				return;
			}

			updatedRecords.push(response.data);
		}

		this.s.bulkMovingKanban = false;

		if (updatedRecords.length > 0) {
			const updatedById = new Map(updatedRecords.map((record) => [record.id, record]));

			this.s.records = this.s.records.map((record) => updatedById.get(record.id) ?? record);
			this.s.filterPreviewRecords = this.s.filterPreviewRecords.map(
				(record) => updatedById.get(record.id) ?? record
			);

			if (this.s.selectedRecord && updatedById.has(this.s.selectedRecord.id)) {
				this.s.selectedRecord = updatedById.get(this.s.selectedRecord.id) ?? this.s.selectedRecord;
			}
		}

		this.clearKanbanSelection();
		toast.success(
			translate('forms.kanbanBulkMoved', { values: { count: selectedRecords.length } })
		);
		await this.fp.loaders.loadAggregates();
	};

	handleKanbanDragStart = (event: DragEvent, record: FormRecord) => {
		this.s.draggingKanbanRecordId = record.id;
		event.dataTransfer?.setData('text/plain', record.id);

		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
		}
	};

	handleKanbanDragEnd = () => {
		this.s.draggingKanbanRecordId = null;
	};

	handleKanbanDragOver = (event: DragEvent, groupLabel: string) => {
		if (!this.s.draggingKanbanRecordId || !this.s.kanbanGroupField) return;

		const record = this.s.records.find((item) => item.id === this.s.draggingKanbanRecordId);

		if (!record || !this.kanbanMoveAllowed(record, groupLabel)) {
			if (event.dataTransfer) event.dataTransfer.dropEffect = 'none';

			return;
		}

		event.preventDefault();

		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
	};

	handleKanbanDrop = async (event: DragEvent, groupLabel: string) => {
		event.preventDefault();

		const recordId = event.dataTransfer?.getData('text/plain') || this.s.draggingKanbanRecordId;

		this.s.draggingKanbanRecordId = null;

		if (!recordId || !this.s.kanbanGroupField) return;

		const record = this.s.records.find((item) => item.id === recordId);

		if (!record) return;

		await this.updateKanbanRecordField(record, this.s.kanbanGroupField, groupLabel);
	};

	viewGroupField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'group_by')
		);

		if (configured) return configured;

		const statusLike = this.s.visibleFields.find((field) =>
			['single_select', 'boolean'].includes(field.type ?? 'text')
		);

		if (statusLike) return statusLike;

		return this.s.visibleFields[0] ?? null;
	};

	viewSwimlaneField = (): FormField | null => {
		const configuredKey = viewConfigString(this.s.selectedView, 'swimlane_by');

		if (!configuredKey || configuredKey === this.s.kanbanGroupField?.key) return null;

		return this.fp.viewRecords.fieldByKey(configuredKey);
	};

	kanbanWipLimits = (): Record<string, number> => {
		const raw = this.s.selectedView?.config?.wip_limits;

		if (!isRecord(raw)) return {};

		const limits: Record<string, number> = {};

		for (const [key, value] of Object.entries(raw)) {
			const limit = typeof value === 'number' ? value : Number(value);

			if (Number.isFinite(limit) && limit >= 0) {
				limits[key] = Math.floor(limit);
			}
		}

		return limits;
	};

	kanbanWipLimit = (label: string): number | null => {
		const limit = this.kanbanWipLimits()[label];

		return typeof limit === 'number' ? limit : null;
	};

	kanbanMoveAllowed = (record: FormRecord, targetValue: string): boolean => {
		if (!this.s.kanbanGroupField) return false;
		if (String(record.values[this.s.kanbanGroupField.key] ?? '') === targetValue) return true;

		const limit = this.kanbanWipLimit(targetValue);

		if (limit === null) return true;

		const count = this.kanbanColumnRecordCount(targetValue);

		return count < limit;
	};

	kanbanColumnRecordCount = (groupLabel: string): number => {
		if (!this.s.kanbanGroupField) return 0;

		const kanbanGroupField = this.s.kanbanGroupField;
		return this.s.records.filter(
			(item) => String(item.values[kanbanGroupField.key] ?? '') === groupLabel
		).length;
	};

	kanbanWipLabel = (groupLabel: string, count: number): string => {
		const limit = this.kanbanWipLimit(groupLabel);

		return limit === null
			? translate('forms.groupRecordCount', { values: { count } })
			: translate('forms.kanbanWipCount', { values: { count, limit } });
	};

	kanbanWipExceeded = (groupLabel: string, count: number): boolean => {
		const limit = this.kanbanWipLimit(groupLabel);

		return limit !== null && count >= limit;
	};

	kanbanColumnWipExceeded = (groupLabel: string): boolean => {
		return this.kanbanWipExceeded(groupLabel, this.kanbanColumnRecordCount(groupLabel));
	};

	groupKanbanSwimlanes = (
		sourceRecords: FormRecord[]
	): Array<{
		key: string;
		label: string;
		records: FormRecord[];
		groups: Array<{ key: string; label: string; records: FormRecord[] }>;
	}> => {
		if (!this.s.kanbanSwimlaneField) return [];

		return groupRecordsByField(sourceRecords, this.s.kanbanSwimlaneField).map((swimlane) => ({
			...swimlane,
			groups: groupRecordsByField(swimlane.records, this.s.kanbanGroupField)
		}));
	};

	kanbanSwimlaneRows = (): Array<{
		key: string;
		label: string;
		records: FormRecord[];
		groups: Array<{ key: string; label: string; records: FormRecord[] }>;
	}> => {
		if (this.s.kanbanSwimlaneField) return this.s.kanbanSwimlanes;

		return [
			{
				key: '__all__',
				label: '',
				records: this.s.viewRecords,
				groups: this.s.kanbanGroups
			}
		];
	};
}
