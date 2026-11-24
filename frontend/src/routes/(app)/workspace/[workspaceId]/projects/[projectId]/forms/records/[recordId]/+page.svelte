<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { writable } from 'svelte/store';
	import CreatorPluginLink from '#lib/components/plugins/CreatorPluginLink.svelte';
	import RecordPluginPanels from '#lib/components/plugins/RecordPluginPanels.svelte';
	import RecordPluginActions from '#lib/components/plugins/RecordPluginActions.svelte';
	import type { RecordPluginActionSurface } from '#lib/plugins/record-entries.js';
	import {
		Activity,
		ArrowLeft,
		FileDown,
		Link,
		MessageSquare,
		Shield,
		TableProperties,
		Workflow
	} from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		formsApi,
		type BusinessEvent,
		type FormAttachment,
		type FormChildRecord,
		type FormField,
		type FormPermissions,
		type FormRecordComment,
		type FormRecordDetail,
		type FormRecordLink,
		type UniversalForm
	} from '#lib/api/forms.js';
	import Button from '#lib/ui/button/Button.svelte';
	import FieldValue from '#lib/components/forms/FieldValue.svelte';
	import {
		buildDetailSections,
		headerFields,
		sectionFieldsForRecord,
		type DetailSectionDensity
	} from '#lib/forms/detail-layout.js';
	import type { FieldDisplayContext } from '#lib/forms/field-display.js';
	import { flowLineageView } from '#lib/forms/flow-lineage.js';
	import { toast } from '#lib/stores/toast.js';
	import { isMissingResult } from '#lib/utils/api-helpers.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { safeImageUrl, safeLinkUrl } from '#lib/utils/safe-url.js';
	import { locale, t } from 'svelte-i18n';

	type DetailLayoutSectionDensity = DetailSectionDensity;

	type DetailPageWidgetKey = 'children' | 'links' | 'comments' | 'attachments' | 'events';
	type DetailPageWidgetRegion = 'sidebar' | 'below';
	type DetailPageWidgetDensity = 'comfortable' | 'compact';

	const detailPageWidgetKeys = new Set<DetailPageWidgetKey>([
		'children',
		'links',
		'comments',
		'attachments',
		'events'
	]);
	const recordPluginActions: RecordPluginActionSurface = writable(null);
	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam(page.params.projectId, 'projectId');
	const recordId = requireRouteParam(page.params.recordId, 'recordId');
	let loading = $state(true);
	let errorMessage = $state('');
	/** The record is gone or hidden (403/404), as opposed to a failed request. */
	let recordMissing = $state(false);
	let record = $state<FormRecordDetail | null>(null);
	let form = $state<UniversalForm | null>(null);
	const pluginRecordInRoute = $derived(
		record?.id === page.params.recordId &&
			record?.project_id === page.params.projectId &&
			form?.workspace_id === page.params.workspaceId
	);
	const flowLineage = $derived(record ? flowLineageView(record.flow_lineage) : null);
	let permissions = $state<FormPermissions | null>(null);
	let links = $state<FormRecordLink[]>([]);
	let children = $state<FormChildRecord[]>([]);
	let comments = $state<FormRecordComment[]>([]);
	let attachments = $state<FormAttachment[]>([]);
	let attachmentSignedUrls = $state<Record<string, string>>({});
	let events = $state<BusinessEvent[]>([]);
	let childForms = $state<Record<string, UniversalForm>>({});
	let commentDraft = $state('');
	let commentSubmitting = $state(false);

	const fields = $derived(form ? parseFields(form.schema) : []);
	const recordEvents = $derived(
		events.filter(
			(event) =>
				event.aggregate_type === 'form_record' &&
				event.aggregate_id === record?.id &&
				event.project_id === projectId
		)
	);
	const activeAttachments = $derived(attachments.filter((attachment) => !attachment.archived_at));
	const attachmentFields = $derived(
		fields.filter(
			(field) =>
				['attachment', 'image'].includes(fieldType(field)) && field.visibility?.detail !== false
		)
	);
	const pageWidgetKeys = $derived(detailPageWidgetKeysFromLayout(form?.detail_layout));
	const pageWidgetRegion = $derived(detailPageWidgetRegionFromLayout(form?.detail_layout));
	const pageFieldColumns = $derived(detailPageFieldColumnsFromLayout(form?.detail_layout));
	const detailHeader = $derived(headerFields(form, fields));
	const detailHeaderSubtitleField = $derived(detailHeader.subtitle);
	const detailHeaderBadgeField = $derived(detailHeader.badge);
	const detailHighlightFields = $derived(detailHeader.highlights);
	const detailSections = $derived(
		buildDetailSections(
			form,
			fields,
			{ main: $t('forms.detailSectionMain'), other: $t('forms.detailSectionAdditional') },
			pageFieldColumns
		)
	);
	const fieldCtx = $derived<FieldDisplayContext>({
		locale: $locale ?? 'en',
		formKey: form?.key ?? '',
		members: new Map(),
		attachmentsByField: activeAttachments.reduce<Record<string, FormAttachment[]>>(
			(groups, item) => {
				(groups[item.field_key] ??= []).push(item);
				return groups;
			},
			{}
		),
		signedUrls: attachmentSignedUrls
	});
	const memberRecordScope = $derived(
		permissions?.effective.record_scope === 'owned' ? 'owned' : 'all'
	);
	const memberHiddenFieldLabels = $derived(fields.filter(memberFieldHidden).map(fieldLabel));
	const memberLockedFieldLabels = $derived(fields.filter(memberFieldLocked).map(fieldLabel));

	onMount(() => {
		void loadDetail();
	});

	function isRecord(input: unknown): input is Record<string, unknown> {
		return typeof input === 'object' && input !== null && !Array.isArray(input);
	}

	function detailPageWidgetKey(value: unknown): DetailPageWidgetKey | null {
		return typeof value === 'string' && detailPageWidgetKeys.has(value as DetailPageWidgetKey)
			? (value as DetailPageWidgetKey)
			: null;
	}

	function defaultDetailPageWidgetKeys(): DetailPageWidgetKey[] {
		return ['children', 'links', 'comments', 'attachments', 'events'];
	}

	function detailPageWidgetKeysFromLayout(
		layout: Record<string, unknown> | null | undefined
	): DetailPageWidgetKey[] {
		const rawWidgets = isRecord(layout) && Array.isArray(layout.widgets) ? layout.widgets : [];
		if (rawWidgets.length === 0) return defaultDetailPageWidgetKeys();
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local scratch collection, not reactive state
		const seen = new Set<DetailPageWidgetKey>();
		const keys: DetailPageWidgetKey[] = [];
		for (const rawWidget of rawWidgets) {
			const key = isRecord(rawWidget)
				? detailPageWidgetKey(rawWidget.key)
				: detailPageWidgetKey(rawWidget);
			if (!key || seen.has(key)) continue;
			seen.add(key);
			if (!isRecord(rawWidget) || rawWidget.enabled !== false) keys.push(key);
		}
		return keys;
	}

	function detailPageWidgetRegionFromLayout(
		layout: Record<string, unknown> | null | undefined
	): DetailPageWidgetRegion {
		const page = isRecord(layout?.page) ? layout.page : {};
		return page.widget_region === 'below' ? 'below' : 'sidebar';
	}

	function detailPageFieldColumnsFromLayout(
		layout: Record<string, unknown> | null | undefined
	): 1 | 2 {
		const page = isRecord(layout?.page) ? layout.page : {};
		const rawColumns = page.field_columns;
		const columns =
			typeof rawColumns === 'number'
				? rawColumns
				: typeof rawColumns === 'string'
					? Number(rawColumns)
					: NaN;
		return columns === 1 ? 1 : 2;
	}

	function keepDetailSectionOpen(event: Event, collapsible: boolean) {
		if (collapsible) return;
		const details = event.currentTarget;
		if (details instanceof HTMLDetailsElement && !details.open) details.open = true;
	}

	function detailLayoutSectionContainerClass(density: DetailLayoutSectionDensity): string {
		const padding = density === 'compact' ? 'p-3' : 'p-4';
		return `rounded-card border border-line bg-raised ${padding} `;
	}

	function detailLayoutSectionGridClass(
		columns: 1 | 2 | 3,
		density: DetailLayoutSectionDensity
	): string {
		const gap = density === 'compact' ? 'gap-3' : 'gap-4';
		if (columns === 1) return `mt-4 grid ${gap}`;
		if (columns === 3) return `mt-4 grid ${gap} md:grid-cols-2 xl:grid-cols-3`;
		return `mt-4 grid ${gap} md:grid-cols-2`;
	}

	function detailLayoutSectionFieldClass(field: FormField, columns: 1 | 2 | 3): string {
		if (fieldType(field) !== 'child_table' || columns === 1) return 'min-w-0';
		return columns === 3 ? 'min-w-0 md:col-span-2 xl:col-span-3' : 'min-w-0 md:col-span-2';
	}

	function detailPageWidgetConfig(key: DetailPageWidgetKey): Record<string, unknown> | null {
		const rawWidgets =
			isRecord(form?.detail_layout) && Array.isArray(form.detail_layout.widgets)
				? form.detail_layout.widgets
				: [];
		const rawWidget = rawWidgets.find((widget) => isRecord(widget) && widget.key === key);
		return isRecord(rawWidget) ? rawWidget : null;
	}

	function detailPageWidgetTitle(key: DetailPageWidgetKey, fallback: string): string {
		const rawWidget = detailPageWidgetConfig(key);
		const title =
			isRecord(rawWidget) && typeof rawWidget.title === 'string' ? rawWidget.title.trim() : '';
		return title || fallback;
	}

	function detailPageWidgetDescription(key: DetailPageWidgetKey): string {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && typeof rawWidget.description === 'string'
			? rawWidget.description.trim()
			: '';
	}

	function detailPageWidgetEmptyText(key: DetailPageWidgetKey, fallback: string): string {
		const rawWidget = detailPageWidgetConfig(key);
		const emptyText =
			isRecord(rawWidget) && typeof rawWidget.empty_text === 'string'
				? rawWidget.empty_text.trim()
				: '';
		return emptyText || fallback;
	}

	function detailPageWidgetShowCount(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.show_count === true;
	}

	function detailPageWidgetHideEmpty(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.hide_empty === true;
	}

	function detailPageWidgetCollapsible(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.collapsible === true;
	}

	function detailPageWidgetDefaultCollapsed(key: DetailPageWidgetKey): boolean {
		const rawWidget = detailPageWidgetConfig(key);
		return (
			isRecord(rawWidget) && rawWidget.collapsible === true && rawWidget.default_collapsed === true
		);
	}

	function detailPageWidgetOpen(key: DetailPageWidgetKey): boolean {
		return !detailPageWidgetCollapsible(key) || !detailPageWidgetDefaultCollapsed(key);
	}

	function detailPageWidgetDensity(key: DetailPageWidgetKey): DetailPageWidgetDensity {
		const rawWidget = detailPageWidgetConfig(key);
		return isRecord(rawWidget) && rawWidget.density === 'compact' ? 'compact' : 'comfortable';
	}

	function detailPageWidgetContainerClass(key: DetailPageWidgetKey): string {
		const padding = detailPageWidgetDensity(key) === 'compact' ? 'p-3' : 'p-4';
		return `rounded-card border border-line bg-raised ${padding} `;
	}

	function detailPageWidgetSummaryClass(key: DetailPageWidgetKey): string {
		const margin = detailPageWidgetDensity(key) === 'compact' ? 'mb-2' : 'mb-3';
		return detailPageWidgetCollapsible(key)
			? `${margin} flex cursor-pointer list-none items-start gap-2`
			: `${margin} flex cursor-default list-none items-start gap-2`;
	}

	function detailPageWidgetBodyClass(key: DetailPageWidgetKey): string {
		return detailPageWidgetDensity(key) === 'compact' ? 'space-y-1.5' : 'space-y-2';
	}

	function keepDetailWidgetOpen(event: Event, collapsible: boolean) {
		if (collapsible) return;
		const details = event.currentTarget;
		if (details instanceof HTMLDetailsElement && !details.open) details.open = true;
	}

	function detailPageWidgetLimit(key: DetailPageWidgetKey, fallback: number): number {
		const rawWidget = detailPageWidgetConfig(key);
		const rawLimit = isRecord(rawWidget) ? rawWidget.limit : null;
		const numericLimit =
			typeof rawLimit === 'number'
				? rawLimit
				: typeof rawLimit === 'string'
					? Number(rawLimit)
					: NaN;
		if (!Number.isFinite(numericLimit)) return fallback;
		const limit = Math.trunc(numericLimit);
		return limit >= 1 && limit <= 50 ? limit : fallback;
	}

	function attachmentWidgetItemCount(): number {
		if (attachmentFields.length === 0) return activeAttachments.length;
		return attachmentFields.reduce(
			(count, field) =>
				count + fieldAttachments(field.key).length + recordAttachmentValueUrls(field).length,
			0
		);
	}

	function parseFields(schema: Record<string, unknown>): FormField[] {
		const rawFields = Array.isArray(schema.fields) ? schema.fields : [];
		return rawFields
			.filter(isRecord)
			.map((field) => {
				const rawType = typeof field.type === 'string' ? field.type : 'text';
				return {
					field_id: typeof field.field_id === 'string' ? field.field_id : undefined,
					key: typeof field.key === 'string' ? field.key : '',
					label: typeof field.label === 'string' ? field.label : undefined,
					type: (rawType === 'select' ? 'single_select' : rawType) as FormField['type'],
					required: Boolean(field.required),
					options: Array.isArray(field.options)
						? field.options.filter((item): item is string => typeof item === 'string')
						: [],
					option_colors: isRecord(field.option_colors)
						? Object.fromEntries(
								Object.entries(field.option_colors).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					option_disabled: Array.isArray(field.option_disabled)
						? field.option_disabled.filter((item): item is string => typeof item === 'string')
						: undefined,
					option_archived: Array.isArray(field.option_archived)
						? field.option_archived.filter((item): item is string => typeof item === 'string')
						: undefined,
					option_descriptions: isRecord(field.option_descriptions)
						? Object.fromEntries(
								Object.entries(field.option_descriptions).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					option_groups: isRecord(field.option_groups)
						? Object.fromEntries(
								Object.entries(field.option_groups).filter(
									(entry): entry is [string, string] => typeof entry[1] === 'string'
								)
							)
						: undefined,
					amount: isRecord(field.amount) ? (field.amount as FormField['amount']) : undefined,
					visibility: isRecord(field.visibility)
						? (field.visibility as FormField['visibility'])
						: undefined,
					relation: isRecord(field.relation)
						? (field.relation as FormField['relation'])
						: undefined,
					attachment: isRecord(field.attachment)
						? (field.attachment as FormField['attachment'])
						: undefined
				};
			})
			.filter((field) => field.key);
	}

	function fieldType(field: FormField): string {
		return field.type ?? 'text';
	}

	function fieldLabel(field: FormField): string {
		return field.label?.trim() || field.key.replaceAll('_', ' ');
	}

	function permissionFieldList(labels: string[]): string {
		return labels.length > 0 ? labels.join(', ') : $t('forms.noFieldPolicyEffects');
	}

	function formatDate(value: string): string {
		return new Date(value).toLocaleString($locale === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function memberFieldHidden(field: FormField): boolean {
		return permissions?.effective.fields?.[field.key]?.read === false;
	}

	function memberFieldLocked(field: FormField): boolean {
		return permissions?.effective.fields?.[field.key]?.write === false;
	}

	function attachmentManifestUrls(value: unknown): string[] {
		if (!value) return [];
		if (typeof value === 'string') return value.trim() ? [value.trim()] : [];
		if (Array.isArray(value)) return value.flatMap((item) => attachmentManifestUrls(item));
		if (isRecord(value)) {
			if (typeof value.url === 'string') return attachmentManifestUrls(value.url);
			if (typeof value.href === 'string') return attachmentManifestUrls(value.href);
		}
		return [];
	}

	function fieldAttachments(fieldKey: string): FormAttachment[] {
		return activeAttachments.filter((attachment) => attachment.field_key === fieldKey);
	}

	function attachmentSignedUrl(attachment: FormAttachment): string {
		return safeLinkUrl(attachmentSignedUrls[attachment.id] || attachment.url);
	}

	function attachmentThumbnailUrl(attachment: FormAttachment): string {
		return safeImageUrl(attachment.thumbnail_url || attachment.url);
	}

	async function loadAttachmentSignedUrls(items: FormAttachment[]) {
		const active = items.filter((attachment) => !attachment.archived_at && attachment.url);
		if (active.length === 0) {
			attachmentSignedUrls = {};
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
		attachmentSignedUrls = Object.fromEntries(
			entries.filter((entry): entry is [string, string] => Array.isArray(entry))
		);
	}

	function recordAttachmentValueUrls(field: FormField): string[] {
		if (!record) return [];
		const metadataUrls = new Set(fieldAttachments(field.key).map((attachment) => attachment.url));
		return attachmentManifestUrls(record.values[field.key]).filter((url) => !metadataUrls.has(url));
	}

	function attachmentIsPreviewableImage(attachment: FormAttachment): boolean {
		return (
			attachment.content_type.startsWith('image/') ||
			/\.(png|jpe?g|webp|gif)$/i.test(attachment.file_name)
		);
	}

	function childFormName(child: FormChildRecord): string {
		return childForms[child.record.form_id]?.name ?? child.record.form_id;
	}

	function eventTitle(event: BusinessEvent): string {
		if (typeof event.payload?.title === 'string') return event.payload.title;
		return event.event_type;
	}

	function eventJson(value: unknown): string {
		return JSON.stringify(value ?? {}, null, 2);
	}

	async function refreshRecord() {
		const response = await formsApi.getRecord(recordId);
		if (response.code === 0 && response.data) record = response.data;
	}

	async function loadDetail() {
		loading = true;
		errorMessage = '';
		recordMissing = false;
		const recordResponse = await formsApi.getRecord(recordId);
		if (recordResponse.code !== 0 || !recordResponse.data) {
			errorMessage = recordResponse.message;
			recordMissing = isMissingResult(recordResponse);
			// A missing record is announced in the tip slot; a failed request by the block's own alert.
			if (recordMissing) toast.error(errorMessage);
			loading = false;
			return;
		}
		record = recordResponse.data;

		const formResponse = await formsApi.get(recordResponse.data.form_id);
		if (formResponse.code !== 0 || !formResponse.data) {
			errorMessage = formResponse.message;
			loading = false;
			return;
		}
		form = formResponse.data;

		const [
			permissionResponse,
			linkResponse,
			childResponse,
			commentResponse,
			attachmentResponse,
			eventResponse
		] = await Promise.all([
			formsApi.getPermissions(formResponse.data.id),
			formsApi.listRecordLinks(recordResponse.data.id),
			formsApi.listRecordChildren(recordResponse.data.id, { per_page: 100 }),
			formsApi.listRecordComments(recordResponse.data.id, { per_page: 20 }),
			formsApi.listAttachments(formResponse.data.id, {
				record_id: recordResponse.data.id,
				per_page: 100
			}),
			formsApi.listEvents(formResponse.data.id)
		]);

		if (permissionResponse.code === 0 && permissionResponse.data)
			permissions = permissionResponse.data;
		if (linkResponse.code === 0 && linkResponse.data) links = linkResponse.data;
		if (childResponse.code === 0 && childResponse.data) children = childResponse.data.items;
		if (commentResponse.code === 0 && commentResponse.data) comments = commentResponse.data.items;
		if (attachmentResponse.code === 0 && attachmentResponse.data) {
			attachments = attachmentResponse.data.items;
			await loadAttachmentSignedUrls(attachments);
		}
		if (eventResponse.code === 0 && eventResponse.data) events = eventResponse.data.items;

		const childFormIds = [...new Set(children.map((child) => child.record.form_id))].filter(
			(formId) => formId !== formResponse.data?.id
		);
		const childFormResponses = await Promise.all(
			childFormIds.map((formId) => formsApi.get(formId))
		);
		childForms = Object.fromEntries(
			childFormResponses
				.map((response) => response.data)
				.filter((item): item is UniversalForm => Boolean(item))
				.map((item) => [item.id, item])
		);

		loading = false;
	}

	async function handleCreateComment() {
		if (!record || !form) return;
		const body = commentDraft.trim();
		if (!body) return;
		commentSubmitting = true;
		const response = await formsApi.createRecordComment(record.id, {
			body,
			metadata: { source: 'web', route: 'record_detail', form_id: form.id, form_key: form.key }
		});
		commentSubmitting = false;
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));
			return;
		}
		comments = [response.data, ...comments.filter((comment) => comment.id !== response.data?.id)];
		commentDraft = '';
		toast.success($t('forms.recordCommentAdded'));
	}

	const documentTitle = $derived(
		$t('pageTitle.formRecordWithTitle', {
			values: { title: record?.title ?? $t('forms.recordTitle') }
		})
	);
</script>

<svelte:head>
	<title>{documentTitle}</title>
</svelte:head>

<div class="min-h-screen bg-canvas px-4 py-6 sm:px-6 lg:px-8">
	<div class="mx-auto max-w-7xl space-y-5">
		<a
			href={form && record
				? resolve(
						`/(app)/workspace/[workspaceId]/projects/[projectId]/forms?form=${form.id}&record=${record.id}`,
						{ workspaceId, projectId }
					)
				: resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
						workspaceId,
						projectId
					})}
			class="inline-flex items-center gap-2 text-sm font-medium text-fg-muted hover:text-fg"
			data-durable-record-detail-back
		>
			<ArrowLeft class="h-4 w-4" aria-hidden="true" />
			{$t('common.back')}
		</a>

		{#if loading}
			<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
				{$t('common.loading')}
			</div>
		{:else if errorMessage}
			{#if recordMissing}
				<StateBlock
					kind="notFound"
					place={$t('workbench.forms').toLowerCase()}
					action={{
						run: () =>
							void goto(
								resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
									workspaceId,
									projectId
								})
							)
					}}
					data-durable-record-detail-error=""
				/>
			{:else}
				<StateBlock
					kind="loadFailed"
					message={errorMessage}
					action={{ run: () => void loadDetail() }}
					data-durable-record-detail-error=""
				/>
			{/if}
		{:else if record && form}
			<header
				class="border-b border-line pb-5"
				data-durable-record-detail-route={record.id}
				data-durable-record-detail-form={form.id}
			>
				<div class="flex flex-wrap items-start justify-between gap-4">
					<div class="min-w-0">
						<div
							class="flex flex-wrap items-center gap-2 text-xs font-medium uppercase text-fg-muted"
						>
							<span>{form.name}</span>
							<span class="font-mono">{form.key}</span>
						</div>
						<h1 class="mt-2 truncate text-2xl font-semibold text-fg">
							{record.title}
						</h1>
						{#if detailHeaderSubtitleField}
							<p
								class="mt-1 flex flex-wrap items-center gap-2 text-sm text-fg-muted"
								data-durable-record-detail-header-subtitle={`${detailHeaderSubtitleField.key}:${record.id}`}
							>
								<span class="text-xs font-medium uppercase text-fg-muted">
									{fieldLabel(detailHeaderSubtitleField)}
								</span>
								<FieldValue
									field={detailHeaderSubtitleField}
									value={record.values[detailHeaderSubtitleField.key]}
									variant="cell"
									ctx={fieldCtx}
								/>
							</p>
						{/if}
						<p class="mt-1 font-mono text-xs text-fg-muted">{record.id}</p>
					</div>
					<div class="grid gap-2 text-right text-xs text-fg-muted">
						{#if detailHeaderBadgeField}
							<div
								class="flex justify-end"
								data-durable-record-detail-header-badge={`${detailHeaderBadgeField.key}:${record.id}`}
							>
								<FieldValue
									field={detailHeaderBadgeField}
									value={record.values[detailHeaderBadgeField.key]}
									variant="cell"
									ctx={fieldCtx}
								/>
							</div>
						{/if}
						<span>{$t('forms.updatedAt', { values: { date: formatDate(record.updated_at) } })}</span
						>
						<span>v{record.schema_version ?? form.schema_version}</span>
						{#if pluginRecordInRoute}
							{#if form.key === 'order_line'}
								<CreatorPluginLink
									{workspaceId}
									{projectId}
									formKey={form.key}
									slot="form.record.actions"
								/>
							{/if}
							<RecordPluginActions recordId={record.id} surface={recordPluginActions} />
						{/if}
					</div>
				</div>
			</header>

			{#if permissions}
				<section
					class="rounded-card border border-line bg-surface px-4 py-3 text-sm"
					data-permission-state-summary
					data-durable-record-detail-permission-state={record.id}
					data-permission-state-mode="durable-detail"
					data-permission-state-scope={memberRecordScope}
					data-permission-state-hidden-fields={memberHiddenFieldLabels.join('|')}
					data-permission-state-locked-fields={memberLockedFieldLabels.join('|')}
				>
					<div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
						<div class="flex min-w-0 items-center gap-2">
							<Shield class="h-4 w-4 shrink-0 text-fg" aria-hidden="true" />
							<div class="min-w-0">
								<p class="font-medium text-fg">
									{$t('forms.permissionState')}
								</p>
								<p class="truncate text-xs text-fg-muted">
									{$t('forms.permissionStateDescription')}
								</p>
							</div>
						</div>
						<div class="grid gap-2 text-xs sm:grid-cols-3 lg:min-w-[520px]">
							<div class="rounded-card border border-line bg-raised px-3 py-2">
								<span class="block font-medium uppercase text-fg-muted">
									{$t('forms.recordScope')}
								</span>
								<span class="mt-1 block text-fg">
									{$t(`forms.recordScopes.${memberRecordScope}`)}
								</span>
							</div>
							<div class="rounded-card border border-line bg-raised px-3 py-2">
								<span class="block font-medium uppercase text-warning">
									{$t('forms.hiddenFieldEffect')}
								</span>
								<span class="mt-1 block truncate text-fg">
									{permissionFieldList(memberHiddenFieldLabels)}
								</span>
							</div>
							<div class="rounded-card border border-line bg-raised px-3 py-2">
								<span class="block font-medium uppercase text-fg-muted">
									{$t('forms.lockedFieldEffect')}
								</span>
								<span class="mt-1 block truncate text-fg">
									{permissionFieldList(memberLockedFieldLabels)}
								</span>
							</div>
						</div>
					</div>
				</section>
			{/if}

			{#if detailHighlightFields.length > 0}
				<section
					class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4"
					data-durable-record-detail-highlights={record.id}
					data-durable-record-detail-highlight-order={detailHighlightFields
						.map((field) => field.key)
						.join('|')}
				>
					{#each detailHighlightFields as field (field.key)}
						<div
							class="min-w-0 rounded-card border border-line bg-raised p-4"
							data-durable-record-detail-highlight={`${record.id}:${field.key}`}
						>
							<div class="truncate text-xs font-medium uppercase text-fg-muted">
								{fieldLabel(field)}
							</div>

							<div class="mt-2 break-words text-sm font-semibold text-fg">
								<FieldValue
									{field}
									value={record.values[field.key]}
									variant="panel"
									ctx={fieldCtx}
								/>
							</div>
						</div>
					{/each}
				</section>
			{/if}

			<div
				class={pageWidgetRegion === 'below'
					? 'space-y-5'
					: 'grid gap-5 lg:grid-cols-[minmax(0,1fr)_360px]'}
				data-durable-record-detail-page-composition={`${record.id}:${pageWidgetRegion}`}
			>
				<main class="space-y-4" data-durable-record-detail-layout={record.id}>
					{#if pluginRecordInRoute}
						{#if form.key === 'order_line'}
							<CreatorPluginLink
								{workspaceId}
								{projectId}
								formKey={form.key}
								slot="form.record.detail"
							/>
						{/if}
						{#key record.id}
							<RecordPluginPanels
								recordId={record.id}
								surface={recordPluginActions}
								onApplied={() => void refreshRecord()}
							/>
						{/key}
					{/if}
					{#each detailSections as section, index (index)}
						<details
							class={detailLayoutSectionContainerClass(section.density)}
							open={!section.collapsible || !section.defaultCollapsed}
							ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
							data-durable-record-detail-section={`${record.id}:${section.key}`}
							data-durable-record-detail-section-density={`${record.id}:${section.key}:${section.density}`}
							data-durable-record-detail-section-collapsible={`${record.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
							data-durable-record-detail-section-open={`${record.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
						>
							<summary
								class={section.collapsible
									? 'cursor-pointer list-none'
									: 'cursor-default list-none'}
							>
								<h2 class="inline text-xs font-semibold uppercase text-fg-muted">
									{section.title}
								</h2>
							</summary>
							{#if section.description}
								<p
									class="mt-2 text-sm text-fg-muted"
									data-durable-record-detail-section-description={`${record.id}:${section.key}`}
								>
									{section.description}
								</p>
							{/if}
							<dl
								class={detailLayoutSectionGridClass(section.columns, section.density)}
								data-durable-record-detail-field-columns={`${record.id}:${section.columns}`}
								data-durable-record-detail-section-columns={`${record.id}:${section.key}:${section.columns}`}
							>
								{#each sectionFieldsForRecord(section, record) as field (field.key)}
									<div
										class={detailLayoutSectionFieldClass(field, section.columns)}
										data-durable-record-detail-field={`${record.id}:${field.key}`}
									>
										<dt
											class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted"
										>
											<span>{fieldLabel(field)}</span>
											{#if memberFieldHidden(field)}
												<span
													class="rounded-[var(--radius-sm)] bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
													data-durable-record-detail-permission-hidden={field.key}
												>
													{$t('forms.fieldHiddenBadge')}
												</span>
											{/if}
											{#if memberFieldLocked(field)}
												<span
													class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
													data-durable-record-detail-permission-locked={field.key}
												>
													{$t('forms.fieldLockedBadge')}
												</span>
											{/if}
										</dt>
										<dd class="mt-1 break-words text-sm text-fg">
											{#if fieldType(field) === 'child_table'}
												<span class="text-fg-muted">{$t('forms.childRecords')}</span>
											{:else}
												<FieldValue
													{field}
													value={record.values[field.key]}
													variant="panel"
													ctx={fieldCtx}
												/>
											{/if}
										</dd>
									</div>
								{/each}
							</dl>
						</details>
					{/each}
				</main>

				<aside
					class={pageWidgetRegion === 'below' ? 'grid gap-4 lg:grid-cols-2' : 'space-y-4'}
					data-durable-record-detail-widget-order={pageWidgetKeys.join('|')}
					data-durable-record-detail-widget-region={pageWidgetRegion}
				>
					{#each pageWidgetKeys as widgetKey, index (index)}
						{#if widgetKey === 'children'}
							{@const childrenWidgetDescription = detailPageWidgetDescription('children')}
							{#if !detailPageWidgetHideEmpty('children') || children.length > 0}
								<details
									class={detailPageWidgetContainerClass('children')}
									open={detailPageWidgetOpen('children')}
									ontoggle={(event) =>
										keepDetailWidgetOpen(event, detailPageWidgetCollapsible('children'))}
									data-durable-record-detail-children={record.id}
									data-durable-record-detail-child-count={children.length}
									data-durable-record-detail-widget-limit={`children:${detailPageWidgetLimit('children', 50)}`}
									data-durable-record-detail-widget-collapsible={`children:${record.id}:${detailPageWidgetCollapsible('children')}:${detailPageWidgetDefaultCollapsed('children')}`}
									data-durable-record-detail-widget-density={`children:${record.id}:${detailPageWidgetDensity('children')}`}
									data-durable-record-detail-widget-open={`children:${record.id}:${detailPageWidgetOpen('children')}`}
								>
									<summary class={detailPageWidgetSummaryClass('children')}>
										<TableProperties class="h-4 w-4 text-fg" aria-hidden="true" />
										<div class="min-w-0">
											<div class="flex min-w-0 flex-wrap items-center gap-2">
												<h2
													class="text-sm font-semibold text-fg"
													data-durable-record-detail-widget-title={`children:${record.id}`}
												>
													{detailPageWidgetTitle('children', $t('forms.childRecords'))}
												</h2>
												{#if detailPageWidgetShowCount('children')}
													<span
														class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-[11px] text-fg-muted"
														data-durable-record-detail-widget-count-badge={`children:${record.id}`}
													>
														{children.length}
													</span>
												{/if}
											</div>
											{#if childrenWidgetDescription}
												<p
													class="mt-1 text-xs leading-5 text-fg-muted"
													data-durable-record-detail-widget-description={`children:${record.id}`}
												>
													{childrenWidgetDescription}
												</p>
											{/if}
										</div>
									</summary>
									{#if children.length > 0}
										<div class="overflow-x-auto">
											<table class="min-w-full divide-y divide-line text-xs">
												<thead>
													<tr>
														<th class="px-3 py-2 text-left font-medium text-fg-muted">
															{$t('forms.recordTitle')}
														</th>
														<th class="px-3 py-2 text-left font-medium text-fg-muted">
															{$t('forms.recordLinks')}
														</th>
													</tr>
												</thead>
												<tbody class="divide-y divide-line">
													{#each children.slice(0, detailPageWidgetLimit('children', 50)) as child (child.link_id)}
														<tr data-durable-record-detail-child={child.record.id}>
															<td class="px-3 py-2 font-medium text-fg">
																{child.record.title}
															</td>
															<td class="px-3 py-2 text-fg-muted">
																{childFormName(child)} · {child.relation_key}
															</td>
														</tr>
													{/each}
												</tbody>
											</table>
										</div>
									{:else}
										<p
											class="text-sm text-fg-muted"
											data-durable-record-detail-widget-empty={`children:${record.id}`}
										>
											{detailPageWidgetEmptyText('children', $t('forms.noChildRecords'))}
										</p>
									{/if}
								</details>
							{/if}
						{:else if widgetKey === 'links'}
							{@const linksWidgetDescription = detailPageWidgetDescription('links')}
							{#if !detailPageWidgetHideEmpty('links') || links.length > 0}
								<details
									class={detailPageWidgetContainerClass('links')}
									open={detailPageWidgetOpen('links')}
									ontoggle={(event) =>
										keepDetailWidgetOpen(event, detailPageWidgetCollapsible('links'))}
									data-durable-record-detail-links={record.id}
									data-durable-record-detail-link-count={links.length}
									data-durable-record-detail-widget-limit={`links:${detailPageWidgetLimit('links', 50)}`}
									data-durable-record-detail-widget-collapsible={`links:${record.id}:${detailPageWidgetCollapsible('links')}:${detailPageWidgetDefaultCollapsed('links')}`}
									data-durable-record-detail-widget-density={`links:${record.id}:${detailPageWidgetDensity('links')}`}
									data-durable-record-detail-widget-open={`links:${record.id}:${detailPageWidgetOpen('links')}`}
								>
									<summary class={detailPageWidgetSummaryClass('links')}>
										<Link class="h-4 w-4 text-fg" aria-hidden="true" />
										<div class="min-w-0">
											<div class="flex min-w-0 flex-wrap items-center gap-2">
												<h2
													class="text-sm font-semibold text-fg"
													data-durable-record-detail-widget-title={`links:${record.id}`}
												>
													{detailPageWidgetTitle('links', $t('forms.recordLinks'))}
												</h2>
												{#if detailPageWidgetShowCount('links')}
													<span
														class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-[11px] text-fg-muted"
														data-durable-record-detail-widget-count-badge={`links:${record.id}`}
													>
														{links.length}
													</span>
												{/if}
											</div>
											{#if linksWidgetDescription}
												<p
													class="mt-1 text-xs leading-5 text-fg-muted"
													data-durable-record-detail-widget-description={`links:${record.id}`}
												>
													{linksWidgetDescription}
												</p>
											{/if}
										</div>
									</summary>
									<div class={detailPageWidgetBodyClass('links')}>
										{#each links.slice(0, detailPageWidgetLimit('links', 50)) as item (item.id)}
											<div class="rounded-card border border-line px-3 py-2 text-xs">
												<div class="font-medium text-fg">
													{item.relation_key} / {item.relation_type}
												</div>

												<div class="mt-1 truncate font-mono text-fg-muted">
													{item.target_type}:{item.target_id}
												</div>
											</div>
										{/each}
										{#if links.length === 0}
											<p
												class="text-sm text-fg-muted"
												data-durable-record-detail-widget-empty={`links:${record.id}`}
											>
												{detailPageWidgetEmptyText('links', $t('forms.noLinkedData'))}
											</p>
										{/if}
									</div>
								</details>
							{/if}
						{:else if widgetKey === 'comments'}
							{@const commentsWidgetDescription = detailPageWidgetDescription('comments')}
							{#if !detailPageWidgetHideEmpty('comments') || comments.length > 0}
								<details
									class={detailPageWidgetContainerClass('comments')}
									open={detailPageWidgetOpen('comments')}
									ontoggle={(event) =>
										keepDetailWidgetOpen(event, detailPageWidgetCollapsible('comments'))}
									data-durable-record-detail-comments={record.id}
									data-durable-record-detail-comment-count={comments.length}
									data-durable-record-detail-widget-limit={`comments:${detailPageWidgetLimit('comments', 5)}`}
									data-durable-record-detail-widget-collapsible={`comments:${record.id}:${detailPageWidgetCollapsible('comments')}:${detailPageWidgetDefaultCollapsed('comments')}`}
									data-durable-record-detail-widget-density={`comments:${record.id}:${detailPageWidgetDensity('comments')}`}
									data-durable-record-detail-widget-open={`comments:${record.id}:${detailPageWidgetOpen('comments')}`}
								>
									<summary class={detailPageWidgetSummaryClass('comments')}>
										<MessageSquare class="h-4 w-4 text-fg" aria-hidden="true" />
										<div class="min-w-0">
											<div class="flex min-w-0 flex-wrap items-center gap-2">
												<h2
													class="text-sm font-semibold text-fg"
													data-durable-record-detail-widget-title={`comments:${record.id}`}
												>
													{detailPageWidgetTitle('comments', $t('forms.recordComments'))}
												</h2>
												{#if detailPageWidgetShowCount('comments')}
													<span
														class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-[11px] text-fg-muted"
														data-durable-record-detail-widget-count-badge={`comments:${record.id}`}
													>
														{comments.length}
													</span>
												{/if}
											</div>
											{#if commentsWidgetDescription}
												<p
													class="mt-1 text-xs leading-5 text-fg-muted"
													data-durable-record-detail-widget-description={`comments:${record.id}`}
												>
													{commentsWidgetDescription}
												</p>
											{/if}
										</div>
									</summary>
									<div class={detailPageWidgetBodyClass('comments')}>
										{#each comments.slice(0, detailPageWidgetLimit('comments', 5)) as comment (comment.id)}
											<div
												class="rounded-card border border-line px-3 py-2 text-xs"
												data-durable-record-detail-comment={comment.id}
											>
												<div class="flex flex-wrap items-center justify-between gap-2">
													<span class="font-medium text-fg">
														{comment.author_name || comment.author_email || $t('people.someone')}
													</span>
													<span class="font-mono text-[11px] text-fg-muted">
														{formatDate(comment.created_at)}
													</span>
												</div>
												<p class="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-fg">
													{comment.body}
												</p>
											</div>
										{/each}
										{#if comments.length === 0}
											<p
												class="text-sm text-fg-muted"
												data-durable-record-detail-widget-empty={`comments:${record.id}`}
											>
												{detailPageWidgetEmptyText('comments', $t('forms.noRecordComments'))}
											</p>
										{/if}
										<textarea
											class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
											bind:value={commentDraft}
											placeholder={$t('forms.recordCommentPlaceholder')}
											data-durable-record-detail-comment-input={record.id}></textarea>
										<span data-durable-record-detail-comment-submit={record.id}>
											<Button
												size="sm"
												variant="secondary"
												loading={commentSubmitting}
												disabled={!commentDraft.trim()}
												onclick={handleCreateComment}
											>
												{$t('forms.addComment')}
											</Button>
										</span>
									</div>
								</details>
							{/if}
						{:else if widgetKey === 'attachments'}
							{@const attachmentsWidgetDescription = detailPageWidgetDescription('attachments')}
							{#if !detailPageWidgetHideEmpty('attachments') || attachmentWidgetItemCount() > 0}
								<details
									class={detailPageWidgetContainerClass('attachments')}
									open={detailPageWidgetOpen('attachments')}
									ontoggle={(event) =>
										keepDetailWidgetOpen(event, detailPageWidgetCollapsible('attachments'))}
									data-durable-record-detail-attachments={record.id}
									data-durable-record-detail-attachment-count={attachmentWidgetItemCount()}
									data-durable-record-detail-widget-collapsible={`attachments:${record.id}:${detailPageWidgetCollapsible('attachments')}:${detailPageWidgetDefaultCollapsed('attachments')}`}
									data-durable-record-detail-widget-density={`attachments:${record.id}:${detailPageWidgetDensity('attachments')}`}
									data-durable-record-detail-widget-open={`attachments:${record.id}:${detailPageWidgetOpen('attachments')}`}
								>
									<summary class={detailPageWidgetSummaryClass('attachments')}>
										<FileDown class="h-4 w-4 text-fg" aria-hidden="true" />
										<div class="min-w-0">
											<div class="flex min-w-0 flex-wrap items-center gap-2">
												<h2
													class="text-sm font-semibold text-fg"
													data-durable-record-detail-widget-title={`attachments:${record.id}`}
												>
													{detailPageWidgetTitle('attachments', $t('forms.attachmentConfig'))}
												</h2>
												{#if detailPageWidgetShowCount('attachments')}
													<span
														class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-[11px] text-fg-muted"
														data-durable-record-detail-widget-count-badge={`attachments:${record.id}`}
													>
														{attachmentWidgetItemCount()}
													</span>
												{/if}
											</div>
											{#if attachmentsWidgetDescription}
												<p
													class="mt-1 text-xs leading-5 text-fg-muted"
													data-durable-record-detail-widget-description={`attachments:${record.id}`}
												>
													{attachmentsWidgetDescription}
												</p>
											{/if}
										</div>
									</summary>
									<div
										class={detailPageWidgetDensity('attachments') === 'compact'
											? 'space-y-2'
											: 'space-y-3'}
									>
										{#each attachmentFields as field (field.key)}
											{@const metadataAttachments = fieldAttachments(field.key)}
											{@const valueUrls = recordAttachmentValueUrls(field)}
											<div
												class="rounded-card border border-line px-3 py-2 text-xs"
												data-durable-record-detail-attachment-field={field.key}
												data-durable-record-detail-attachment-field-count={`${field.key}:${metadataAttachments.length + valueUrls.length}`}
											>
												<div class="mb-2 font-medium text-fg">
													{fieldLabel(field)}
												</div>
												<div class={detailPageWidgetBodyClass('attachments')}>
													{#each metadataAttachments as attachment (attachment.id)}
														<div
															class="flex flex-wrap items-center gap-2 rounded-control bg-surface px-2 py-1.5"
															data-durable-record-detail-attachment={attachment.id}
														>
															{#if attachmentIsPreviewableImage(attachment)}
																<img
																	src={attachmentThumbnailUrl(attachment)}
																	alt={attachment.file_name}
																	class="h-10 w-10 rounded-control object-cover ring-1 ring-line"
																/>
															{/if}
															<div class="min-w-0 flex-1">
																<div class="truncate font-medium text-fg">
																	{attachment.file_name}
																</div>

																<div class="truncate font-mono text-[11px] text-fg-muted">
																	{attachment.url}
																</div>
															</div>

															<!-- eslint-disable svelte/no-navigation-without-resolve -- attachment and user-supplied URLs are not app routes -->
															<a
																href={attachmentSignedUrl(attachment) || undefined}
																target="_blank"
																rel="noreferrer"
																class="rounded-control px-2 py-1 font-medium text-fg hover:bg-surface"
																data-durable-record-detail-attachment-signed-url={attachment.id}
															>
																{$t('forms.attachmentOpen')}
															</a>
															<!-- eslint-enable svelte/no-navigation-without-resolve -->
														</div>
													{/each}
													{#each valueUrls as url, index (index)}
														{#if safeLinkUrl(url)}
															<!-- eslint-disable svelte/no-navigation-without-resolve -- attachment and user-supplied URLs are not app routes -->
															<a
																href={safeLinkUrl(url)}
																target="_blank"
																rel="noreferrer"
																class="block truncate rounded-control bg-surface px-2 py-1.5 font-mono text-[11px] text-fg"
																data-durable-record-detail-attachment-value={`${field.key}:${url}`}
															>
																{url}
															</a>
															<!-- eslint-enable svelte/no-navigation-without-resolve -->
														{:else}
															<span
																class="block truncate rounded-control bg-surface px-2 py-1.5 font-mono text-[11px] text-warning"
																data-durable-record-detail-attachment-value={`${field.key}:${url}`}
																data-durable-record-detail-attachment-value-blocked={field.key}
															>
																{url}
															</span>
														{/if}
													{/each}
													{#if metadataAttachments.length === 0 && valueUrls.length === 0}
														<p class="text-sm text-fg-muted">
															{detailPageWidgetEmptyText(
																'attachments',
																$t('forms.noRecordAttachments')
															)}
														</p>
													{/if}
												</div>
											</div>
										{/each}
										{#if attachmentFields.length === 0 && activeAttachments.length > 0}
											{#each activeAttachments as attachment (attachment.id)}
												<div
													class="flex flex-wrap items-center gap-2 rounded-card border border-line px-3 py-2 text-xs"
													data-durable-record-detail-attachment={attachment.id}
												>
													{#if attachmentIsPreviewableImage(attachment)}
														<img
															src={attachmentThumbnailUrl(attachment)}
															alt={attachment.file_name}
															class="h-10 w-10 rounded-control object-cover ring-1 ring-line"
														/>
													{/if}
													<div class="min-w-0 flex-1">
														<div class="truncate font-medium text-fg">
															{attachment.file_name}
														</div>

														<div class="truncate font-mono text-[11px] text-fg-muted">
															{attachment.field_key} · {attachment.url}
														</div>
													</div>

													<!-- eslint-disable svelte/no-navigation-without-resolve -- attachment and user-supplied URLs are not app routes -->
													<a
														href={attachmentSignedUrl(attachment) || undefined}
														target="_blank"
														rel="noreferrer"
														class="rounded-control px-2 py-1 font-medium text-fg hover:bg-surface"
														data-durable-record-detail-attachment-signed-url={attachment.id}
													>
														{$t('forms.attachmentOpen')}
													</a>
													<!-- eslint-enable svelte/no-navigation-without-resolve -->
												</div>
											{/each}
										{/if}
										{#if attachmentFields.length === 0}
											{#if activeAttachments.length === 0}
												<p
													class="text-sm text-fg-muted"
													data-durable-record-detail-widget-empty={`attachments:${record.id}`}
												>
													{detailPageWidgetEmptyText(
														'attachments',
														$t('forms.noRecordAttachments')
													)}
												</p>
											{/if}
										{/if}
									</div>
								</details>
							{/if}
						{:else if widgetKey === 'events'}
							{@const eventsWidgetDescription = detailPageWidgetDescription('events')}
							{#if !detailPageWidgetHideEmpty('events') || recordEvents.length > 0}
								<details
									class={detailPageWidgetContainerClass('events')}
									open={detailPageWidgetOpen('events')}
									ontoggle={(event) =>
										keepDetailWidgetOpen(event, detailPageWidgetCollapsible('events'))}
									data-durable-record-detail-events={record.id}
									data-durable-record-detail-event-count={recordEvents.length}
									data-durable-record-detail-widget-limit={`events:${detailPageWidgetLimit('events', 5)}`}
									data-durable-record-detail-widget-collapsible={`events:${record.id}:${detailPageWidgetCollapsible('events')}:${detailPageWidgetDefaultCollapsed('events')}`}
									data-durable-record-detail-widget-density={`events:${record.id}:${detailPageWidgetDensity('events')}`}
									data-durable-record-detail-widget-open={`events:${record.id}:${detailPageWidgetOpen('events')}`}
								>
									<summary class={detailPageWidgetSummaryClass('events')}>
										<Activity class="h-4 w-4 text-fg" aria-hidden="true" />
										<div class="min-w-0">
											<div class="flex min-w-0 flex-wrap items-center gap-2">
												<h2
													class="text-sm font-semibold text-fg"
													data-durable-record-detail-widget-title={`events:${record.id}`}
												>
													{detailPageWidgetTitle('events', $t('forms.timelineSourceEvents'))}
												</h2>
												{#if detailPageWidgetShowCount('events')}
													<span
														class="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-[11px] text-fg-muted"
														data-durable-record-detail-widget-count-badge={`events:${record.id}`}
													>
														{recordEvents.length}
													</span>
												{/if}
											</div>
											{#if eventsWidgetDescription}
												<p
													class="mt-1 text-xs leading-5 text-fg-muted"
													data-durable-record-detail-widget-description={`events:${record.id}`}
												>
													{eventsWidgetDescription}
												</p>
											{/if}
										</div>
									</summary>
									<div class={detailPageWidgetBodyClass('events')}>
										{#each recordEvents.slice(0, detailPageWidgetLimit('events', 5)) as event (event.id)}
											<details
												class="rounded-card border border-line px-3 py-2 text-xs"
												data-durable-record-detail-event={event.id}
											>
												<summary class="cursor-pointer list-none">
													<div class="flex flex-wrap items-start justify-between gap-2">
														<div class="min-w-0">
															<div class="truncate font-medium text-fg">
																{eventTitle(event)}
															</div>

															<div class="mt-1 font-mono text-[11px] text-fg-muted">
																{event.event_type}
															</div>
														</div>

														<span class="font-mono text-[11px] text-fg-muted"
															>{formatDate(event.created_at)}</span
														>
													</div>
												</summary>
												<pre
													class="mt-3 max-h-40 overflow-auto rounded-card bg-surface p-2 font-mono text-[11px] text-fg-muted">{eventJson(
														event.payload
													)}</pre>
											</details>
										{/each}
										{#if recordEvents.length === 0}
											<p
												class="text-sm text-fg-muted"
												data-durable-record-detail-widget-empty={`events:${record.id}`}
											>
												{detailPageWidgetEmptyText('events', $t('forms.noRecordEvents'))}
											</p>
										{/if}
									</div>
								</details>
							{/if}
						{/if}
					{/each}
					{#if record && flowLineage}
						<details
							class="rounded-card border border-line bg-raised p-4"
							open
							data-testid="record-flow-lineage"
							data-reference-count={flowLineage.references.length}
							data-conversion-count={flowLineage.conversions.length}
						>
							<summary
								class="mb-3 flex min-h-11 cursor-pointer list-none items-start gap-2 rounded-row"
							>
								<Workflow class="h-4 w-4 text-fg" aria-hidden="true" />
								<div class="min-w-0">
									<h2 class="text-sm font-semibold text-fg">
										{$t('forms.flowLineage.title')}
									</h2>
									<p class="mt-1 text-xs leading-5 text-fg-muted">
										{$t('forms.flowLineage.description')}
									</p>
								</div>
							</summary>
							<div class="space-y-3">
								<section aria-labelledby="record-flow-lineage-references">
									<h3
										id="record-flow-lineage-references"
										class="text-xs font-semibold uppercase tracking-wide text-fg-muted"
									>
										{$t('forms.flowLineage.references')}
									</h3>
									<ul class="mt-2 space-y-2">
										{#each flowLineage.references as row (row.key)}
											<li
												class="rounded-[var(--radius-sm)] border border-line px-3 py-2 text-xs"
												data-testid="record-flow-lineage-reference"
												data-available={row.available}
											>
												{#if row.available}
													<a
														href={resolve('/(app)/flow/[objectId]', {
															objectId: row.sourceObjectId
														})}
														class="inline-flex min-h-11 items-center font-medium text-fg hover:underline"
														data-testid="record-flow-lineage-source"
														>{row.sourceTitle ?? $t('forms.flowLineage.untitled')}</a
													>

													<div class="flex flex-wrap gap-x-3 text-fg-muted">
														<span data-testid="record-flow-lineage-mode"
															>{$t(`forms.flowLineage.mode.${row.mode}`)}</span
														>
														<span
															>{$t('forms.flowLineage.addedAt', {
																values: { date: formatDate(row.createdAt) }
															})}</span
														>
													</div>
												{:else}
													<span class="text-fg-muted" data-testid="record-flow-lineage-unavailable">
														{$t('forms.flowLineage.sourceUnavailable')}
													</span>
												{/if}
											</li>
										{:else}
											<li class="text-sm text-fg-muted">
												{$t('forms.flowLineage.noReferences')}
											</li>
										{/each}
									</ul>
								</section>
								<section aria-labelledby="record-flow-lineage-conversions">
									<h3
										id="record-flow-lineage-conversions"
										class="text-xs font-semibold uppercase tracking-wide text-fg-muted"
									>
										{$t('forms.flowLineage.conversions')}
									</h3>
									<ul class="mt-2 space-y-2">
										{#each flowLineage.conversions as row (row.key)}
											<li
												class="rounded-[var(--radius-sm)] border border-line px-3 py-2 text-xs"
												data-testid="record-flow-lineage-conversion"
												data-available={row.available}
											>
												{#if row.available}
													<a
														href={resolve('/(app)/flow/[objectId]', {
															objectId: row.sourceObjectId
														})}
														class="inline-flex min-h-11 items-center font-medium text-fg hover:underline"
														data-testid="record-flow-lineage-source"
														>{row.sourceTitle ?? $t('forms.flowLineage.untitled')}</a
													>

													<div class="flex flex-wrap items-center gap-x-3 text-fg-muted">
														<span data-testid="record-flow-lineage-status"
															>{$t(`forms.flowLineage.status.${row.status}`)}</span
														>
														<span
															>{$t('forms.flowLineage.startedAt', {
																values: { date: formatDate(row.createdAt) }
															})}</span
														>
														{#if row.linkedJobId}
															<a
																href={resolve(
																	'/(app)/workspace/[workspaceId]/flow/conversions/[jobId]',
																	{
																		workspaceId,
																		jobId: row.linkedJobId
																	}
																)}
																class="inline-flex min-h-11 items-center text-fg hover:underline"
																data-testid="record-flow-lineage-job"
															>
																{$t('forms.flowLineage.openJob')}
															</a>
														{/if}
													</div>
												{:else}
													<span class="text-fg-muted" data-testid="record-flow-lineage-unavailable">
														{$t('forms.flowLineage.sourceUnavailable')}
													</span>
												{/if}
											</li>
										{:else}
											<li class="text-sm text-fg-muted">
												{$t('forms.flowLineage.noConversions')}
											</li>
										{/each}
									</ul>
								</section>
								<p class="text-xs leading-5 text-fg-muted">
									{$t('forms.flowLineage.readOnlyNote')}
								</p>
							</div>
						</details>
					{/if}
				</aside>
			</div>
		{/if}
	</div>
</div>
