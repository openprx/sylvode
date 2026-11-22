<script lang="ts" module>
	import type { FormField } from '#lib/api/forms.js';

	function isPlain(value: unknown): value is Record<string, unknown> {
		return typeof value === 'object' && value !== null && !Array.isArray(value);
	}

	function stringMap(value: unknown): Record<string, string> | undefined {
		if (!isPlain(value)) return undefined;
		return Object.fromEntries(
			Object.entries(value).filter(
				(entry): entry is [string, string] => typeof entry[1] === 'string'
			)
		);
	}

	function stringList(value: unknown): string[] | undefined {
		return Array.isArray(value)
			? value.filter((item): item is string => typeof item === 'string')
			: undefined;
	}

	/** The fields of a form schema, as the record panel reads them (the same shape the record page builds). */
	export function schemaFields(schema: Record<string, unknown>): FormField[] {
		const raw = Array.isArray(schema.fields) ? schema.fields : [];
		return raw.filter(isPlain).flatMap((field): FormField[] => {
			if (typeof field.key !== 'string' || !field.key) return [];
			const rawType = typeof field.type === 'string' ? field.type : 'text';
			const object = <K extends keyof FormField>(key: string) =>
				(isPlain(field[key]) ? field[key] : undefined) as FormField[K];
			return [
				{
					field_id: typeof field.field_id === 'string' ? field.field_id : undefined,
					key: field.key,
					label: typeof field.label === 'string' ? field.label : undefined,
					type: (rawType === 'select' ? 'single_select' : rawType) as FormField['type'],
					required: Boolean(field.required),
					options: stringList(field.options) ?? [],
					option_colors: stringMap(field.option_colors),
					option_disabled: stringList(field.option_disabled),
					option_archived: stringList(field.option_archived),
					option_descriptions: stringMap(field.option_descriptions),
					option_groups: stringMap(field.option_groups),
					amount: object<'amount'>('amount'),
					placeholder: typeof field.placeholder === 'string' ? field.placeholder : undefined,
					help_text: typeof field.help_text === 'string' ? field.help_text : undefined,
					default_value: field.default_value,
					validation: object<'validation'>('validation'),
					visibility: object<'visibility'>('visibility'),
					conditional: object<'conditional'>('conditional'),
					policy: object<'policy'>('policy'),
					formula: object<'formula'>('formula'),
					relation: object<'relation'>('relation'),
					attachment: object<'attachment'>('attachment')
				}
			];
		});
	}

	/** Address of the full record page: where "Edit" and "Open full page" lead in the host's main area. */
	export function recordPageHref(workspaceId: string, projectId: string, recordId: string): string {
		return `/workspace/${workspaceId}/projects/${projectId}/forms/records/${recordId}`;
	}
</script>

<script lang="ts">
	/**
	 * The record panel as a hosted surface: loads one record and renders `RecordSidePanel` in its
	 * `hosted` form (body only). Title and actions go to the frame's host through `useSurface`; the
	 * panel never navigates the window. The pilot is read-only: editing happens on the full page,
	 * which the host opens in its main area.
	 */
	import { locale, t } from 'svelte-i18n';
	import { onMount } from 'svelte';
	import { writable } from 'svelte/store';
	import type { RecordPluginActionSurface } from '#lib/plugins/record-entries.js';
	import {
		formsApi,
		type FormAttachment,
		type FormChildRecord,
		type FormPermissions,
		type FormRecord,
		type UniversalForm
	} from '#lib/api/forms.js';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { authStore } from '#lib/stores/auth.js';
	import { buildDetailSections, headerFields } from '#lib/forms/detail-layout.js';
	import type { FieldDisplayContext } from '#lib/forms/field-display.js';
	import { useSurface } from '#lib/surface/use-surface.svelte.js';
	import type { SurfaceTarget } from '#lib/surface/context.js';
	import RecordSidePanel, { type PanelStatus } from './RecordSidePanel.svelte';

	interface Props {
		workspaceId: string;
		projectId: string;
		recordId: string;
	}

	let { workspaceId, projectId, recordId }: Props = $props();

	// Plugin panels are not part of the pilot (`plugins.enabled` is false); the panel still needs the store.
	const pluginSurface: RecordPluginActionSurface = writable(null);
	// Stands in for the form until it has loaded; the panel reads it only once a record is ready.
	const NO_FORM: UniversalForm = {
		id: '',
		workspace_id: '',
		project_id: '',
		key: '',
		name: '',
		description: '',
		title_template: '',
		schema: {},
		detail_layout: {},
		schema_version: 0,
		created_at: '',
		updated_at: ''
	};

	let status = $state<PanelStatus>('loading');
	let errorMessage = $state('');
	let record = $state<FormRecord | null>(null);
	let form = $state<UniversalForm | null>(null);
	let permissions = $state<FormPermissions | null>(null);
	let children = $state<FormChildRecord[]>([]);
	let attachments = $state<FormAttachment[]>([]);
	let signedUrls = $state<Record<string, string>>({});
	let members = $state<WorkspaceMember[]>([]);
	let detailsLoading = $state(true);
	let appsOpen = $state(false);

	const fields = $derived(form ? schemaFields(form.schema) : []);
	const sections = $derived(
		buildDetailSections(form, fields, {
			main: $t('forms.detailSectionMain'),
			other: $t('forms.recordPanel.otherFields')
		})
	);
	const header = $derived(headerFields(form, fields));
	const activeAttachments = $derived(attachments.filter((item) => !item.archived_at));
	const ctx = $derived<FieldDisplayContext>({
		locale: $locale ?? 'en',
		formKey: form?.key ?? '',
		members: new Map(members.map((member) => [member.user_id, member])),
		attachmentsByField: activeAttachments.reduce<Record<string, FormAttachment[]>>(
			(groups, item) => {
				(groups[item.field_key] ??= []).push(item);
				return groups;
			},
			{}
		),
		signedUrls
	});
	const pageHref = $derived(recordPageHref(workspaceId, projectId, recordId));

	const autonumber = $derived(fields.find((field) => field.type === 'autonumber') ?? null);
	const identifier = $derived.by(() => {
		if (!record) return undefined;
		const value = autonumber ? record.values?.[autonumber.key] : null;
		return typeof value === 'string' && value.trim() ? value.trim() : `#${record.id.slice(0, 8)}`;
	});

	function canEditRecord(item: FormRecord): boolean {
		const effective = permissions?.effective;
		if (!effective) return false;
		return (
			effective.actions['record.update'] &&
			(effective.record_scope !== 'owned' || item.created_by === $authStore.user?.id)
		);
	}
	const canEdit = $derived(!!record && status === 'ready' && canEditRecord(record));

	function target(id: string): SurfaceTarget {
		return {
			kind: 'record',
			id,
			workspaceId,
			projectId,
			formId: form?.id,
			href: recordPageHref(workspaceId, projectId, id),
			panelHref: form
				? `/workspace/${workspaceId}/projects/${projectId}/forms?form=${form.id}&record=${id}`
				: undefined,
			label: id === record?.id ? record.title : undefined
		};
	}

	const surface = useSurface(() => ({
		title: record?.title ?? '',
		identifier,
		loading: status === 'loading',
		context:
			form && record
				? { segments: [{ id: 'form', label: form.name }], current: record.title }
				: undefined,
		primary: canEdit
			? {
					id: 'edit',
					label: $t('forms.recordPanel.edit'),
					kind: 'primary',
					run: () => surface.open(target(recordId), 'main')
				}
			: undefined,
		secondary:
			status === 'ready'
				? [
						{
							id: 'open-full',
							label: $t('forms.recordPanel.openFullPage'),
							kind: 'secondary',
							run: () => surface.open(target(recordId), 'main')
						}
					]
				: [],
		tip:
			status === 'error'
				? {
						id: 'record-load-error',
						tone: 'danger',
						message: errorMessage || $t('forms.recordPanel.loadError'),
						action: { label: $t('forms.recordPanel.retry'), run: () => void load() }
					}
				: null,
		dirty: false
	}));

	async function loadSignedUrls(items: FormAttachment[]): Promise<void> {
		const active = items.filter((item) => !item.archived_at && item.url);
		const entries = await Promise.all(
			active.map(async (item) => {
				const response = await formsApi.getAttachmentSignedDownload(item.id);
				return response.code === 0 && response.data?.url
					? ([item.id, response.data.url] as [string, string])
					: null;
			})
		);
		signedUrls = Object.fromEntries(entries.filter((entry) => entry !== null));
	}

	async function load(): Promise<void> {
		status = 'loading';
		errorMessage = '';
		detailsLoading = true;
		const recordResponse = await formsApi.getRecord(recordId);
		const found = recordResponse.data;
		if (recordResponse.code !== 0 || !found) {
			status =
				recordResponse.code === 404
					? 'missing'
					: recordResponse.code === 403
						? 'forbidden'
						: 'error';
			errorMessage = status === 'error' ? recordResponse.message : '';
			return;
		}
		// A record of another workspace or project is not shown under this address.
		if (found.workspace_id !== workspaceId || found.project_id !== projectId || found.archived_at) {
			status = 'missing';
			return;
		}
		const formResponse = await formsApi.get(found.form_id);
		if (formResponse.code !== 0 || !formResponse.data) {
			status = formResponse.code === 403 ? 'forbidden' : 'error';
			errorMessage = status === 'error' ? formResponse.message : '';
			return;
		}
		record = found;
		form = formResponse.data;
		status = 'ready';

		// Permissions, children, attachments and members follow in the background.
		const [permissionResponse, childResponse, attachmentResponse, memberResponse] =
			await Promise.all([
				formsApi.getPermissions(formResponse.data.id),
				formsApi.listRecordChildren(found.id, { per_page: 100 }),
				formsApi.listAttachments(formResponse.data.id, { record_id: found.id, per_page: 100 }),
				membersApi.list(workspaceId)
			]);
		if (permissionResponse.code === 0 && permissionResponse.data)
			permissions = permissionResponse.data;
		if (childResponse.code === 0 && childResponse.data) children = childResponse.data.items;
		if (memberResponse.code === 0 && memberResponse.data) members = memberResponse.data.items;
		if (attachmentResponse.code === 0 && attachmentResponse.data) {
			attachments = attachmentResponse.data.items;
			await loadSignedUrls(attachments);
		}
		detailsLoading = false;
	}

	onMount(() => {
		void load();
	});

	function childTable(field: FormField) {
		const relationKey = field.relation?.relation_key || field.key;
		return {
			rows: children.filter((child) => child.relation_key === relationKey),
			columns: [] as FormField[]
		};
	}

	function memberName(userId: string | null | undefined): string {
		return (
			members.find((member) => member.user_id === userId)?.name ??
			$t('forms.fieldValue.unknownMember')
		);
	}
</script>

<div class="record-surface" data-surface="record">
	<RecordSidePanel
		hosted
		bind:appsOpen
		{record}
		{status}
		{errorMessage}
		inView={true}
		form={form ?? NO_FORM}
		{sections}
		{header}
		{ctx}
		{childTable}
		{detailsLoading}
		position={null}
		fullPageHref={pageHref}
		recordHref={(id) => recordPageHref(workspaceId, projectId, id)}
		recordUrl={typeof location === 'undefined' ? pageHref : new URL(pageHref, location.origin).href}
		{memberName}
		{canEdit}
		canDelete={false}
		canRestore={false}
		showConfigBadges={false}
		isHiddenByCondition={() => false}
		plugins={{ workspaceId, projectId, surface: pluginSurface, enabled: false }}
		width={surface.host.width}
		onclose={() => surface.close()}
		onprev={() => {}}
		onnext={() => {}}
		onedit={() => surface.open(target(recordId), 'main')}
		ondelete={() => {}}
		onrestore={() => {}}
		onretry={() => void load()}
		onshowall={() => {}}
		onresize={() => {}}
		onopenrelation={(id, sameForm, event) => {
			void sameForm;
			event.preventDefault();
			surface.open(target(id), 'replace');
		}}
		onpluginapplied={() => {}}
		onfocuslist={() => {}}
		onannounce={() => {}}
		onmoredetails={() => surface.open(target(recordId), 'main')}
	/>
</div>

<style>
	.record-surface {
		display: flex;
		flex-direction: column;
		height: 100dvh;
		min-width: 0;
	}
</style>
