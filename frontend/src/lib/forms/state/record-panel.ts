import { goto } from '$app/navigation';
import { tick } from 'svelte';
import { page } from '$app/state';
import { type FormField, type FormRecord, formsApi } from '#lib/api/forms.js';
import type { GridBadge } from '#lib/components/forms/RecordGrid.svelte';
import * as arrangement from '#lib/forms/view-arrangement.js';
import { toast } from '#lib/stores/toast.js';
import { recordPageSize } from '#lib/forms/page-constants.js';
import {
	fieldHasConditionalHidden,
	fieldHasConditionalReadOnly,
	recordFieldHiddenByCondition,
	recordFieldReadOnlyByCondition
} from '#lib/forms/field-conditions.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class RecordPanelController extends FormsController {
	syncRecordDetailUrl = async (record: FormRecord | null) => {
		const url = new URL(page.url.href);

		if (record) {
			url.searchParams.set('form', this.s.selectedFormId);
			url.searchParams.set('record', record.id);
		} else {
			url.searchParams.delete('record');
			url.searchParams.delete('record_id');

			if (this.s.selectedFormId) {
				url.searchParams.set('form', this.s.selectedFormId);
			} else {
				url.searchParams.delete('form');
				url.searchParams.delete('form_id');
			}
		}

		const next = `${url.pathname}${url.search}${url.hash}`;
		const current = `${page.url.pathname}${page.url.search}${page.url.hash}`;

		if (next !== current) {
			await goto(next, { replaceState: true, reset: false });
		}
	};

	restoreRecordDetailFromUrl = async () => {
		const recordId = this.s.detailUrlRecordId;

		if (recordId && recordId !== this.s.panelRecordId) await this.openRecordPanel(recordId, 'none');
	};

	/** Every view opens a record the same way: in the side panel next to the list. */
	selectRecord = async (record: FormRecord, options: { syncUrl?: boolean } = {}) => {
		await this.openRecordPanel(record, options.syncUrl === false ? 'none' : 'push');
	};

	goToRecordPage = (page: number) => {
		this.s.recordPage = Math.min(Math.max(page, 1), this.s.recordPageCount);
	};

	loadSelectedRecordLinks = async () => {
		if (!this.s.selectedRecord) {
			this.s.selectedRecordLinks = [];
			this.s.selectedRecordChildren = [];
			this.s.selectedRecordComments = [];

			return;
		}

		const response = await formsApi.listRecordLinks(this.s.selectedRecord.id);

		if (response.code === 0 && response.data) {
			this.s.selectedRecordLinks = response.data;
		}
	};

	loadSelectedRecordChildren = async () => {
		if (!this.s.selectedRecord) {
			this.s.selectedRecordChildren = [];

			return;
		}

		const response = await formsApi.listRecordChildren(this.s.selectedRecord.id, { per_page: 100 });

		if (response.code === 0 && response.data) {
			this.s.selectedRecordChildren = response.data.items;
		}
	};

	loadSelectedRecordComments = async () => {
		if (!this.s.selectedRecord) {
			this.s.selectedRecordComments = [];

			return;
		}

		this.s.recordCommentsLoading = true;

		const response = await formsApi.listRecordComments(this.s.selectedRecord.id, { per_page: 20 });

		this.s.recordCommentsLoading = false;

		if (response.code !== 0) {
			this.s.selectedRecordComments = [];
			toast.error(response.message);

			return;
		}

		this.s.selectedRecordComments = response.data?.items ?? [];
	};

	reloadSelectedRecord = async () => {
		if (!this.s.selectedRecord) return;

		const recordId = this.s.selectedRecord.id;
		const response = await formsApi.getRecord(recordId);
		if (this.s.selectedRecord?.id !== recordId) return;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.selectedRecord = response.data;
		this.fp.records.replaceLocalRecord(response.data);
	};

	announce = (message: string, delay = 0) => {
		clearTimeout(this.s.announceTimer);
		this.s.announceTimer = setTimeout(() => (this.s.liveMessage = message), delay);
	};

	recordHref = (recordId: string): string => {
		return `/workspace/${this.s.workspaceId}/projects/${this.s.projectId}/forms/records/${recordId}`;
	};

	canChangeRecord = (record: FormRecord, action: 'record.update' | 'record.delete'): boolean => {
		const effective = this.s.formPermissions?.effective;
		if (!effective) return true;
		return (
			effective.actions[action] &&
			(effective.record_scope !== 'owned' || record.created_by === this.s.currentUserId)
		);
	};

	setRecordUrl = (recordId: string | null, mode: 'push' | 'replace'): Promise<void> => {
		this.s.recordUrlWrite = this.s.recordUrlWrite
			.then(async () => {
				const url = new URL(page.url.href);
				url.searchParams.delete('record_id');
				if (recordId) url.searchParams.set('record', recordId);
				else url.searchParams.delete('record');
				if (recordId) url.searchParams.set('form', this.s.selectedFormId);
				if (url.href === page.url.href) return;
				// eslint-disable-next-line svelte/no-navigation-without-resolve -- same route, query only
				await goto(url, { replace: mode === 'replace', reset: false });
				if (mode === 'push' && page.url.href === url.href) this.s.panelHistoryDepth += 1;
			})
			.catch((error: unknown) => {
				// A superseded navigation must not stop later URL writes.
				console.warn('record URL update was not applied', error);
			});
		return this.s.recordUrlWrite;
	};

	openRecordPanel = async (target: FormRecord | string, history: 'push' | 'replace' | 'none') => {
		const id = typeof target === 'string' ? target : target.id;
		const local =
			typeof target === 'string'
				? (this.s.records.find((record) => record.id === id) ?? null)
				: target;
		this.s.viewToolsExpanded = false;
		if (!this.s.panelRecordId)
			this.s.panelWidth = arrangement.readPanelWidth(this.s.currentUserId) ?? this.s.panelWidth;
		this.s.panelRecordId = id;
		this.s.panelStatus = local ? 'ready' : 'loading';
		this.s.panelError = '';
		this.s.selectedRecord = local;
		const index = this.s.viewRecords.findIndex((record) => record.id === id);
		if (index >= 0) this.s.recordPage = Math.floor(index / recordPageSize) + 1;
		if (history !== 'none') await this.setRecordUrl(id, history);
		if (local && index >= 0)
			this.announce(
				translate('forms.recordPanel.announce', {
					values: { title: local.title, index: index + 1, total: this.s.viewRecords.length }
				}),
				300
			);
		const started = Date.now();
		const response = await formsApi.getRecord(id);
		if (this.s.panelRecordId !== id) return;
		const elapsed = Date.now() - started;
		if (!local && elapsed > 200 && elapsed < 500)
			await new Promise((resolve) => setTimeout(resolve, 500 - elapsed));
		if (response.code === 0 && response.data && !response.data.archived_at) {
			this.s.selectedRecord = response.data;
			this.s.panelStatus = 'ready';
			this.s.panelDetailsLoading = true;
			await Promise.all([
				this.loadSelectedRecordLinks(),
				this.loadSelectedRecordChildren(),
				this.fp.attachments.loadRecordAttachments(id)
			]);
			this.s.panelDetailsLoading = false;
		} else if (response.code === 404 || response.data?.archived_at) this.s.panelStatus = 'missing';
		else if (!local) {
			this.s.panelStatus = response.code === 403 ? 'forbidden' : 'error';
			this.s.panelError = response.message;
		}
	};

	closeRecordPanel = (restoreFocus = true) => {
		const id = this.s.panelRecordId;
		if (!id) return;
		const index = this.s.recordGrid?.rowIndex(id) ?? -1;
		this.s.panelRecordId = null;
		this.s.selectedRecord = null;
		this.s.selectedRecordLinks = [];
		this.s.selectedRecordChildren = [];
		this.announce(translate('forms.recordPanel.announceClosed'));
		void this.s.recordUrlWrite.then(() => {
			if (this.s.panelHistoryDepth > 0) {
				history.go(-this.s.panelHistoryDepth);
				this.s.panelHistoryDepth = 0;
			} else void this.setRecordUrl(null, 'replace');
		});
		if (!restoreFocus) return;
		this.s.pendingRowFocus = id;
		void tick().then(() => {
			if (this.s.recordGrid && this.s.viewRecords.some((record) => record.id === id))
				this.s.recordGrid.focusRow(id);
			else if (this.s.recordGrid) this.s.recordGrid.focusIndex(Math.max(index, 0));
			else document.querySelector<HTMLElement>(`[data-record-open="${CSS.escape(id)}"]`)?.focus();
		});
	};

	stepRecordPanel = (delta: 1 | -1) => {
		const next = this.s.viewRecords[this.s.panelPosition + delta];
		if (!next) return;
		void this.openRecordPanel(next, 'replace');
	};

	gridBadges = (field: FormField, record: FormRecord | null): GridBadge[] => {
		const prefix = record ? 'grid-cell' : 'grid-header';
		const value = record ? `${record.id}:${field.key}` : field.key;
		const badges: GridBadge[] = [];
		if (!record && this.fp.permissions.memberFieldHidden(field))
			badges.push({
				attr: `${prefix}-permission-hidden`,
				value,
				label: translate('forms.fieldHiddenBadge'),
				tone: 'amber'
			});
		if (!record && this.fp.permissions.memberFieldLocked(field))
			badges.push({
				attr: `${prefix}-permission-locked`,
				value,
				label: translate('forms.fieldLockedBadge'),
				tone: 'slate'
			});
		if (record ? recordFieldHiddenByCondition(field, record) : fieldHasConditionalHidden(field))
			badges.push({
				attr: `${prefix}-conditional-hidden`,
				value,
				label: translate('forms.fieldConditionHiddenBadge'),
				tone: 'blue'
			});
		if (record ? recordFieldReadOnlyByCondition(field, record) : fieldHasConditionalReadOnly(field))
			badges.push({
				attr: `${prefix}-conditional-readonly`,
				value,
				label: translate('forms.fieldConditionReadOnlyBadge'),
				tone: 'cyan'
			});
		return badges;
	};
}
