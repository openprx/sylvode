#!/usr/bin/env node
// Mock API and static server for the built forms app, used by the browser smokes
// (smoke-forms-record-list.mjs, smoke-forms-page-interactions.mjs). Prints SMOKE_SERVER_READY <port>
// and serves until it is stopped.
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, normalize, resolve } from 'node:path';

const root = resolve(new URL('..', import.meta.url).pathname);
const buildDir = join(root, 'build');
const requestedSmokeMode = process.env.OPENPR_FORMS_UI_SMOKE_MODE || 'full';
const workspaceId = 'ws-forms-smoke';
const projectId = 'project-forms-smoke';
const now = '2026-05-31T12:00:00.000Z';

let createdRecordPayload = null;
let createdLinkPayload = null;
let createdRecord = null;
let updatedRecordPayload = null;
let updatedRecordPayloads = [];
let updatedViewPayload = null;
let updatedViewPayloads = [];
let updatedFormPayloads = [];
let createdLinks = [];
let orderLineRecordListRequests = [];
let orderLineExportRequests = [];
let viewUpdateCounter = 0;
let createdAttachmentPayloads = [];
let deletedAttachmentIds = [];
let uploadedAttachmentPayloads = [];
let formAttachments = [];
let listedRecordCommentRecordIds = [];
let createdRecordCommentPayloads = [];
let formRecordComments = [
	{
		id: 'comment-record-existing',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: 'form-order-line',
		record_id: 'record-order-line-created',
		author_id: 'user-smoke',
		author_name: 'Smoke Admin',
		author_email: 'smoke@example.com',
		body: 'Kitchen lead reviewed the detail handoff.',
		metadata: { source: 'seed' },
		archived_at: null,
		created_at: now,
		updated_at: now
	}
];

function signedAttachmentUrl(id) {
	return `/api/v1/form-attachments/${id}/download?expires=1893456000&signature=signed-${id}`;
}

function serverAttachmentThumbnailUrl(filename = 'server-counter-ticket.png') {
	return `/api/v1/uploads/thumbnails/thumb-${filename}`;
}
let memberPermissionPolicy = {
	actions: {
		'form.view': true,
		'form.design': true,
		'record.create': true,
		'record.update': true,
		'record.delete': true,
		'record.export': true
	},
	record_scope: 'all',
	fields: {}
};

function applyMemberPermissionSmokePolicy() {
	memberPermissionPolicy = {
		...memberPermissionPolicy,
		record_scope: 'owned',
		fields: {
			...memberPermissionPolicy.fields,
			unit_price: { read: false, write: false },
			quantity: { write: false }
		}
	};
}

const project = {
	id: projectId,
	workspace_id: workspaceId,
	name: 'Restaurant Forms UI',
	key: 'RESTUI',
	description: 'Forms UI browser smoke',
	type_key: 'custom_form',
	type_settings: {},
	created_at: now,
	updated_at: now,
	issue_counts: null
};

const orderLineForm = {
	id: 'form-order-line',
	workspace_id: workspaceId,
	project_id: projectId,
	key: 'order_line',
	name: 'Order Line',
	description: 'Order line item as child table data.',
	icon: null,
	color: null,
	title_template: '{sku_name}',
	schema: {
		version: 'openpr.form.schema.v1',
		fields: [
			{
				field_id: 'fld_order_id',
				key: 'order_id',
				label: 'Order',
				type: 'relation',
				required: true
			},
			{ field_id: 'fld_sku_id', key: 'sku_id', label: 'SKU', type: 'relation', required: true },
			{
				field_id: 'fld_sku_name',
				key: 'sku_name',
				label: 'SKU Name',
				type: 'text',
				required: true
			},
			{
				field_id: 'fld_quantity',
				key: 'quantity',
				label: 'Quantity',
				type: 'integer',
				required: true
			},
			{
				field_id: 'fld_unit_price',
				key: 'unit_price',
				label: 'Unit Price',
				type: 'amount',
				required: true,
				amount: { currency: 'CNY', scale: 2 }
			},
			{
				field_id: 'fld_line_total',
				key: 'line_total',
				label: 'Line Total',
				type: 'amount',
				amount: { currency: 'CNY', scale: 2 }
			},
			{ field_id: 'fld_seat_no', key: 'seat_no', label: 'Seat', type: 'text' },
			{
				field_id: 'fld_status',
				key: 'status',
				label: 'Status',
				type: 'single_select',
				options: ['draft', 'sent_to_kitchen', 'served', 'cancelled'],
				option_colors: {
					draft: '#64748b',
					sent_to_kitchen: '#f59e0b',
					served: '#16a34a',
					cancelled: '#dc2626'
				}
			},
			{ field_id: 'fld_service_date', key: 'service_date', label: 'Service Date', type: 'date' }
		]
	},
	detail_layout: {
		sections: [
			{
				key: 'handoff',
				title: 'Kitchen Handoff',
				fields: ['sku_name', 'quantity', 'seat_no', 'status']
			},
			{ key: 'pricing', title: 'Pricing', fields: ['unit_price', 'line_total'] }
		]
	},
	schema_version: 1,
	archived_at: null,
	created_by: 'user-smoke',
	created_at: now,
	updated_at: now
};

const orderLineViews = [
	{
		id: 'view-order-line-default',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'default_grid',
		name: 'Default Grid',
		view_type: 'grid',
		config: {
			columns: [
				'order_id',
				'sku_name',
				'quantity',
				'unit_price',
				'line_total',
				'seat_no',
				'status'
			],
			is_default: false
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-detail',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'detail_review',
		name: 'Detail Review',
		view_type: 'detail',
		config: {
			columns: ['sku_name', 'quantity', 'unit_price', 'line_total', 'seat_no', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-kanban',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'kitchen_board',
		name: 'Kitchen Board',
		view_type: 'kanban',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'seat_no', 'status'],
			group_by: 'status'
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-pivot',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'seat_status_pivot',
		name: 'Seat Status Pivot',
		view_type: 'pivot',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'seat_no', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-gantt',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'service_gantt',
		name: 'Service Gantt',
		view_type: 'gantt',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status', 'service_date', 'seat_no']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-calendar',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'service_calendar',
		name: 'Service Calendar',
		view_type: 'calendar',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status', 'service_date'],
			date_field: 'service_date'
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-card',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'line_cards',
		name: 'Line Cards',
		view_type: 'card',
		config: {
			columns: ['sku_name', 'quantity', 'unit_price', 'line_total', 'seat_no', 'status'],
			is_default: true
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-timeline',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'update_timeline',
		name: 'Update Timeline',
		view_type: 'timeline',
		config: {
			columns: ['sku_name', 'quantity', 'line_total', 'status']
		},
		created_by: 'user-smoke',
		created_at: now,
		updated_at: now
	},
	{
		id: 'view-order-line-private-other',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: orderLineForm.id,
		key: 'other_private',
		name: 'Other Private',
		view_type: 'grid',
		config: {
			columns: ['sku_name', 'quantity', 'status'],
			visibility: 'private'
		},
		created_by: 'user-other',
		created_at: now,
		updated_at: now
	}
];

const printJobForm = {
	id: 'form-print-job',
	workspace_id: workspaceId,
	project_id: projectId,
	key: 'print_job',
	name: 'Print Job',
	description: 'Kitchen ticket and cashier receipt tasks.',
	icon: null,
	color: null,
	title_template: '{job_type}',
	schema: {
		version: 'openpr.form.schema.v1',
		fields: [
			{ key: 'order_id', label: 'Order', type: 'relation', required: true },
			{
				key: 'job_type',
				label: 'Job Type',
				type: 'single_select',
				required: true,
				options: ['kitchen', 'receipt']
			},
			{
				key: 'status',
				label: 'Status',
				type: 'single_select',
				required: true,
				options: ['pending', 'printing', 'printed', 'failed']
			},
			{ key: 'printer', label: 'Printer', type: 'text' },
			{ key: 'payload', label: 'Payload', type: 'textarea' },
			{ key: 'retry_count', label: 'Retry Count', type: 'integer' }
		]
	},
	detail_layout: {},
	schema_version: 1,
	archived_at: null,
	created_by: 'user-smoke',
	created_at: now,
	updated_at: now
};

const printJobs = [
	{
		id: 'print-job-kitchen',
		workspace_id: workspaceId,
		project_id: projectId,
		form_id: printJobForm.id,
		title: 'Kitchen ticket',
		values: {
			order_id: { record_id: 'order-1' },
			job_type: 'kitchen',
			status: 'printed',
			printer: 'kitchen-01',
			payload: 'Beef Noodles x2',
			retry_count: { type: 'integer', value: 1 }
		},
		source: { type: 'smoke' },
		created_by: 'user-smoke',
		updated_by: 'user-smoke',
		created_at: now,
		updated_at: now
	}
];

const comparisonRecord = {
	id: 'record-order-line-comparison',
	workspace_id: workspaceId,
	project_id: projectId,
	form_id: orderLineForm.id,
	title: 'Green Tea x1',
	values: {
		order_id: { record_id: 'order-1', title: 'Order 1', form_key: 'order' },
		sku_id: { record_id: 'sku-2', title: 'Green Tea', form_key: 'sku' },
		sku_name: 'Green Tea',
		quantity: { type: 'integer', value: 1 },
		unit_price: { type: 'amount', decimal: '5.00', currency: 'CNY', scale: 2 },
		line_total: { type: 'amount', decimal: '5.00', currency: 'CNY', scale: 2 },
		seat_no: '2',
		status: 'served',
		service_date: '2026-05-30',
		service_end_date: '2026-05-30'
	},
	source: { type: 'smoke' },
	created_by: 'user-other',
	updated_by: 'user-other',
	created_at: now,
	updated_at: now
};

function seedCreatedRecord() {
	if (!createdRecord) {
		createdRecord = {
			id: 'record-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			form_id: orderLineForm.id,
			title: 'Beef Noodles x2',
			values: {
				order_id: { record_id: 'order-1', title: 'Order 1', form_key: 'order' },
				sku_id: { record_id: 'sku-1', title: 'Beef Noodles', form_key: 'sku' },
				sku_name: 'Beef Noodles',
				quantity: { type: 'integer', value: 2 },
				unit_price: { type: 'amount', decimal: '9.99', currency: 'CNY', scale: 2 },
				line_total: { type: 'amount', decimal: '19.98', currency: 'CNY', scale: 2 },
				seat_no: '1',
				status: 'sent_to_kitchen',
				service_date: '2026-05-31',
				attachment_12: 'https://assets.example.test/detail-ticket.pdf',
				dish_photo: 'https://assets.example.test/beef-noodles.jpg',
				ticket_no: 'AUTO-000001'
			},
			source: { type: 'web' },
			created_by: 'user-smoke',
			updated_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
	}
	if (!formAttachments.some((attachment) => attachment.id === 'attachment-durable-detail')) {
		formAttachments = [
			{
				id: 'attachment-durable-detail',
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: 'record-order-line-created',
				field_id: 'fld_attachment_12',
				field_key: 'attachment_12',
				file_name: 'detail-ticket.pdf',
				content_type: 'application/pdf',
				byte_size: 2048,
				storage_key: 'smoke/detail-ticket.pdf',
				url: 'https://assets.example.test/detail-ticket.pdf',
				created_by: 'user-smoke',
				archived_at: null,
				created_at: now,
				updated_at: now
			},
			...formAttachments
		];
	}
	return createdRecord;
}

function orderLineEvents() {
	const items = [
		{
			id: 'event-order-line-comparison-updated',
			workspace_id: workspaceId,
			project_id: projectId,
			event_type: 'form.record.updated',
			aggregate_type: 'form_record',
			aggregate_id: comparisonRecord.id,
			actor_id: 'user-other',
			source: { type: 'smoke', origin: 'api' },
			payload: { title: comparisonRecord.title, status: comparisonRecord.values.status },
			metadata: { form_id: orderLineForm.id, form_key: orderLineForm.key },
			correlation_id: null,
			causation_id: null,
			idempotency_key: null,
			created_at: new Date(Date.parse(now) - 60_000).toISOString()
		}
	];
	if (createdRecord) {
		items.unshift({
			id: 'event-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			event_type: 'form.record.created',
			aggregate_type: 'form_record',
			aggregate_id: createdRecord.id,
			actor_id: 'user-smoke',
			source: { type: 'web', origin: 'forms-ui-smoke' },
			payload: { title: createdRecord.title, status: createdRecord.values.status },
			metadata: { form_id: orderLineForm.id, form_key: orderLineForm.key },
			correlation_id: null,
			causation_id: null,
			idempotency_key: 'forms-ui-smoke-created',
			created_at: new Date(Date.parse(now) + 5_000).toISOString()
		});
	}
	return items;
}

function apiResult(data) {
	return { code: 0, message: 'success', data };
}

function nextUpdatedAt() {
	viewUpdateCounter += 1;
	return new Date(Date.parse(now) + viewUpdateCounter * 1000).toISOString();
}

function paginated(items) {
	return { items, total: items.length, page: 1, per_page: items.length, total_pages: 1 };
}

function readableFieldKeys() {
	return Object.entries(memberPermissionPolicy.fields ?? {})
		.filter(([, policy]) => policy?.read === false)
		.map(([fieldKey]) => fieldKey);
}

function filterReadableRecord(record) {
	const hidden = readableFieldKeys();
	if (hidden.length === 0) return record;
	const values = { ...(record.values ?? {}) };
	for (const fieldKey of hidden) delete values[fieldKey];
	return { ...record, values };
}

function viewById(viewId) {
	return orderLineViews.find((view) => view.id === viewId) ?? null;
}

function filterNodeDisabled(node) {
	return Boolean(
		node && typeof node === 'object' && (node.disabled === true || node.enabled === false)
	);
}

function recordMatchesFilter(record, filter) {
	const expected = typeof filter.value === 'string' ? filter.value.trim().toLowerCase() : '';
	const rawValue = record.values[filter.field];
	const actualValue =
		rawValue && typeof rawValue === 'object'
			? typeof rawValue.display === 'string'
				? rawValue.display
				: typeof rawValue.title === 'string'
					? rawValue.title
					: typeof rawValue.decimal === 'string'
						? rawValue.decimal
						: typeof rawValue.value === 'number'
							? String(rawValue.value)
							: JSON.stringify(rawValue)
			: rawValue;
	if (filter.operator === 'not_empty') {
		return String(actualValue ?? '').trim().length > 0;
	}
	if (!expected) return true;
	const actual = String(actualValue ?? '').toLowerCase();
	if (filter.operator === 'equals') return actual === expected;
	if (filter.operator === 'not_equals') return actual !== expected;
	return actual.includes(expected);
}

function recordMatchesFilterNode(record, filter) {
	if (filterNodeDisabled(filter)) return null;
	return recordMatchesFilter(record, filter);
}

function recordMatchesFilterExpression(record, expression) {
	if (!expression || typeof expression !== 'object') return true;
	if (filterNodeDisabled(expression)) return null;
	const filter =
		expression.filter && typeof expression.filter === 'object' ? expression.filter : expression;
	if (typeof filter.field === 'string') return recordMatchesFilterNode(record, filter);
	const rawChildren = Array.isArray(expression.children)
		? expression.children
		: Array.isArray(expression.expressions)
			? expression.expressions
			: [];
	const matches = rawChildren
		.map((child) => recordMatchesFilterExpression(record, child))
		.filter((match) => match !== null);
	if (matches.length === 0) return null;
	return expression.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
}

function orderLineRecordsForUrl(url) {
	let records = createdRecord ? [comparisonRecord, createdRecord] : [comparisonRecord];
	const view = viewById(url.searchParams.get('view_id'));
	if (memberPermissionPolicy.record_scope === 'owned') {
		records = records.filter((record) => record.created_by === 'user-smoke');
	}
	if (view?.config?.filter_expression) {
		records = records.filter(
			(record) => recordMatchesFilterExpression(record, view.config.filter_expression) !== false
		);
	} else {
		const filterGroups = Array.isArray(view?.config?.filter_groups)
			? view.config.filter_groups
			: Array.isArray(view?.config?.filters)
				? [{ logic: 'all', filters: view.config.filters }]
				: view?.config?.filter
					? [{ logic: 'all', filters: [view.config.filter] }]
					: [];
		for (const group of filterGroups) {
			if (filterNodeDisabled(group)) continue;
			const filters = Array.isArray(group.filters) ? group.filters : [];
			records = records.filter((record) => {
				const matches = filters
					.map((filter) => recordMatchesFilterNode(record, filter))
					.filter((match) => match !== null);
				if (matches.length === 0) return true;
				return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
			});
		}
	}
	if (view?.config?.sort?.field === 'quantity') {
		const direction = view.config.sort.direction === 'desc' ? -1 : 1;
		records = [...records].sort((left, right) => {
			const leftValue = Number(left.values.quantity?.value ?? left.values.quantity ?? 0);
			const rightValue = Number(right.values.quantity?.value ?? right.values.quantity ?? 0);
			return (leftValue - rightValue) * direction;
		});
	}
	return records.map(filterReadableRecord);
}

function json(res, status, data) {
	res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
	res.end(JSON.stringify(data));
}

function contentType(filePath) {
	const ext = extname(filePath);
	if (ext === '.html') return 'text/html; charset=utf-8';
	if (ext === '.js') return 'application/javascript; charset=utf-8';
	if (ext === '.css') return 'text/css; charset=utf-8';
	if (ext === '.json') return 'application/json; charset=utf-8';
	if (ext === '.svg') return 'image/svg+xml';
	return 'application/octet-stream';
}

async function readBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	const text = Buffer.concat(chunks).toString('utf8');
	return text ? JSON.parse(text) : {};
}

async function readRawBody(req) {
	const chunks = [];
	for await (const chunk of req) chunks.push(chunk);
	return Buffer.concat(chunks);
}

async function serveStatic(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	let pathname = decodeURIComponent(url.pathname);
	if (pathname === '/') pathname = '/index.html';
	let filePath = normalize(join(buildDir, pathname));
	if (!filePath.startsWith(buildDir) || !existsSync(filePath)) {
		filePath = join(buildDir, 'index.html');
	}

	const data = await readFile(filePath, 'utf8');
	res.writeHead(200, { 'Content-Type': contentType(filePath) });
	res.end(data);
}

async function handler(req, res) {
	const url = new URL(req.url ?? '/', 'http://127.0.0.1');
	const pathname = url.pathname;

	if (pathname === '/smoke-auth-seed') {
		const mode = url.searchParams.get('mode') ?? 'desktop';
		if (mode === 'durable-detail') applyMemberPermissionSmokePolicy();
		const extra =
			mode === 'detail-url' ? '&form=form-order-line&record=record-order-line-created' : '';
		const target =
			mode === 'durable-detail'
				? `/workspace/${workspaceId}/projects/${projectId}/forms/records/record-order-line-created?record_detail_smoke=${mode}`
				: `/workspace/${workspaceId}/projects/${projectId}/forms?forms_smoke=${mode}${extra}`;
		res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
		res.end(`<script>
localStorage.setItem('auth_token', 'smoke-access-token');
localStorage.setItem('refresh_token', 'smoke-refresh-token');
localStorage.setItem('locale', 'en');
location.replace(${JSON.stringify(target)});
</script>`);
		return;
	}

	if (pathname === '/api/v1/auth/me') {
		json(
			res,
			200,
			apiResult({
				user: { id: 'user-smoke', email: 'smoke@example.com', name: 'Smoke Admin', role: 'admin' }
			})
		);
		return;
	}
	if (pathname === '/api/v1/notifications/unread-count') {
		json(res, 200, apiResult({ count: 0 }));
		return;
	}
	if (pathname === '/api/v1/workspaces') {
		json(
			res,
			200,
			apiResult(
				paginated([
					{
						id: workspaceId,
						slug: 'forms-smoke',
						name: 'Forms Smoke',
						created_at: now,
						updated_at: now
					}
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/members`) {
		json(
			res,
			200,
			apiResult(
				paginated([
					{
						user_id: 'user-smoke',
						workspace_id: workspaceId,
						email: 'smoke@example.com',
						name: 'Smoke Admin',
						role: 'owner',
						joined_at: now
					}
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/workspaces/${workspaceId}/projects`) {
		json(res, 200, apiResult(paginated([project])));
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}`) {
		json(res, 200, apiResult(project));
		return;
	}
	if (pathname === `/api/v1/projects/${projectId}/forms`) {
		json(res, 200, apiResult(paginated([orderLineForm, printJobForm])));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/views`) {
		const visibleViews = orderLineViews
			.filter((view) => view.config?.visibility !== 'private' || view.created_by === 'user-smoke')
			.sort((left, right) => {
				if (Boolean(left.config?.is_default) === Boolean(right.config?.is_default)) return 0;
				return left.config?.is_default ? -1 : 1;
			});
		json(res, 200, apiResult(visibleViews));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/field-usage`) {
		const fields = orderLineForm.schema.fields.map((field) => ({
			field_id: field.field_id ?? `fld_${field.key}`,
			field_key: field.key,
			field_type: field.type,
			value_count: field.field_id === 'fld_order_id' || field.field_id === 'fld_sku_name' ? 1 : 0,
			indexed_count: field.field_id === 'fld_order_id' || field.field_id === 'fld_sku_name' ? 1 : 0,
			dependency_count: field.field_id === 'fld_order_id' ? 1 : 0
		}));
		json(
			res,
			200,
			apiResult({ form_id: orderLineForm.id, schema_version: orderLineForm.schema_version, fields })
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/field-dependencies`) {
		const orderField = orderLineForm.schema.fields.find(
			(field) => field.field_id === 'fld_order_id'
		);
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				schema_version: orderLineForm.schema_version,
				dependencies: [
					{
						field_id: 'fld_order_id',
						field_key: orderField?.key ?? 'order_id',
						source_type: 'view',
						source_id: 'view-order-line-default',
						source_key: 'default_grid',
						reason: 'visible column',
						blocking: true
					}
				]
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/events`) {
		json(res, 200, apiResult(paginated(orderLineEvents())));
		return;
	}
	const recordCommentsMatch = pathname.match(/^\/api\/v1\/form-records\/([^/]+)\/comments$/);
	if (recordCommentsMatch) {
		const recordId = decodeURIComponent(recordCommentsMatch[1]);
		if (req.method === 'POST') {
			const body = await readBody(req);
			createdRecordCommentPayloads.push(body);
			const comment = {
				id: `comment-record-created-${createdRecordCommentPayloads.length}`,
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: recordId,
				author_id: 'user-smoke',
				author_name: 'Smoke Admin',
				author_email: 'smoke@example.com',
				body: body.body,
				metadata: body.metadata ?? {},
				archived_at: null,
				created_at: now,
				updated_at: now
			};
			formRecordComments = [comment, ...formRecordComments];
			json(res, 200, apiResult(comment));
			return;
		}
		listedRecordCommentRecordIds.push(recordId);
		json(
			res,
			200,
			apiResult(
				paginated(
					formRecordComments.filter(
						(comment) => comment.record_id === recordId && !comment.archived_at
					)
				)
			)
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/attachments`) {
		if (req.method === 'POST') {
			const body = await readBody(req);
			createdAttachmentPayloads.push(body);
			const attachment = {
				id: `attachment-${createdAttachmentPayloads.length}`,
				workspace_id: workspaceId,
				project_id: projectId,
				form_id: orderLineForm.id,
				record_id: body.record_id ?? null,
				field_id: `fld_${body.field_key}`,
				field_key: body.field_key,
				file_name: body.file_name,
				content_type: body.content_type ?? '',
				byte_size: body.byte_size ?? 0,
				storage_key: body.storage_key,
				url: body.url ?? '',
				thumbnail_url: body.thumbnail_url ?? null,
				media_metadata: body.media_metadata ?? {},
				created_by: 'user-smoke',
				archived_at: null,
				created_at: now,
				updated_at: now
			};
			formAttachments = [attachment, ...formAttachments];
			json(res, 200, apiResult(attachment));
			return;
		}
		const recordId = url.searchParams.get('record_id');
		const fieldKey = url.searchParams.get('field_key');
		const includeArchived = url.searchParams.get('include_archived') === 'true';
		const items = formAttachments.filter(
			(attachment) =>
				(!recordId || attachment.record_id === recordId) &&
				(!fieldKey || attachment.field_key === fieldKey) &&
				(includeArchived || !attachment.archived_at)
		);
		json(res, 200, apiResult(paginated(items)));
		return;
	}
	if (pathname === '/api/v1/upload' && req.method === 'POST') {
		const body = await readRawBody(req);
		uploadedAttachmentPayloads.push({
			content_type: req.headers['content-type'] ?? '',
			byte_size: body.length
		});
		json(
			res,
			200,
			apiResult({
				url: '/api/v1/uploads/server-counter-ticket.png',
				filename: 'server-counter-ticket.png',
				thumbnail_url: serverAttachmentThumbnailUrl()
			})
		);
		return;
	}
	if (pathname === serverAttachmentThumbnailUrl() && req.method === 'GET') {
		res.writeHead(200, { 'Content-Type': 'image/png' });
		res.end(Buffer.from('server-counter-ticket-thumbnail'));
		return;
	}
	if (pathname === '/api/v1/uploads/server-counter-ticket.png' && req.method === 'GET') {
		res.writeHead(200, { 'Content-Type': 'image/png' });
		res.end(Buffer.from('server-counter-ticket'));
		return;
	}
	const attachmentSignedUrlMatch = pathname.match(
		/^\/api\/v1\/form-attachments\/([^/]+)\/signed-url$/
	);
	if (attachmentSignedUrlMatch && req.method === 'GET') {
		const attachmentId = attachmentSignedUrlMatch[1];
		json(
			res,
			200,
			apiResult({
				url: signedAttachmentUrl(attachmentId),
				expires_at: '2030-01-01T00:00:00.000Z'
			})
		);
		return;
	}
	const attachmentDownloadMatch = pathname.match(
		/^\/api\/v1\/form-attachments\/([^/]+)\/download$/
	);
	if (attachmentDownloadMatch && req.method === 'GET') {
		const attachmentId = attachmentDownloadMatch[1];
		const attachment = formAttachments.find((item) => item.id === attachmentId);
		if (!attachment || url.searchParams.get('signature') !== `signed-${attachmentId}`) {
			json(res, 401, { code: 401, message: 'invalid signature', data: null });
			return;
		}
		res.writeHead(302, { Location: attachment.url });
		res.end();
		return;
	}
	const attachmentDeleteMatch = pathname.match(/^\/api\/v1\/form-attachments\/([^/]+)$/);
	if (attachmentDeleteMatch && req.method === 'DELETE') {
		const attachmentId = attachmentDeleteMatch[1];
		deletedAttachmentIds.push(attachmentId);
		formAttachments = formAttachments.map((attachment) =>
			attachment.id === attachmentId
				? { ...attachment, archived_at: now, updated_at: now }
				: attachment
		);
		json(res, 200, apiResult(null));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/relation-targets`) {
		json(
			res,
			200,
			apiResult(
				paginated([
					{
						record_id: 'order-1',
						form_id: 'form-order',
						form_key: 'order',
						form_name: 'Order',
						title: 'Order 1',
						display: 'Order 1',
						values: { order_no: 'Order 1' },
						updated_at: now
					}
				])
			)
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/permissions`) {
		if (req.method === 'PATCH') {
			const body = await readBody(req);
			const memberPolicy = body?.policies?.find(
				(policy) => policy.subject_type === 'role' && policy.subject_id === 'member'
			);
			if (memberPolicy?.policy) memberPermissionPolicy = memberPolicy.policy;
		}
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				policies: [
					{
						id: 'permission-member',
						workspace_id: workspaceId,
						project_id: projectId,
						form_id: orderLineForm.id,
						subject_type: 'role',
						subject_id: 'member',
						policy: memberPermissionPolicy,
						created_at: now,
						updated_at: now
					}
				],
				effective: {
					role: 'member',
					is_bot: false,
					actions: memberPermissionPolicy.actions,
					record_scope: memberPermissionPolicy.record_scope,
					fields: memberPermissionPolicy.fields ?? {}
				}
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}` && req.method === 'PATCH') {
		const body = await readBody(req);
		updatedFormPayloads.push(body);
		if (body.schema) {
			orderLineForm.schema = body.schema;
			orderLineForm.schema_version += 1;
			orderLineForm.updated_at = now;
		}
		if (body.detail_layout && typeof body.detail_layout === 'object') {
			orderLineForm.detail_layout = body.detail_layout;
			orderLineForm.updated_at = now;
		}
		json(res, 200, apiResult(orderLineForm));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}` && req.method === 'GET') {
		json(res, 200, apiResult(orderLineForm));
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}` && req.method === 'GET') {
		json(res, 200, apiResult(printJobForm));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/records/export` && req.method === 'GET') {
		const records = orderLineRecordsForUrl(url);
		const view = viewById(url.searchParams.get('view_id'));
		const exportFormat = url.searchParams.get('format') === 'json' ? 'json' : 'csv';
		const hidden = new Set(readableFieldKeys());
		const columns = (view?.config?.columns ?? ['sku_name', 'quantity', 'status'])
			.filter((key) => !hidden.has(key))
			.map((key) => ({
				key,
				label: orderLineForm.schema.fields.find((field) => field.key === key)?.label ?? key
			}));
		orderLineExportRequests.push({
			view_id: url.searchParams.get('view_id'),
			format: exportFormat,
			rows: records.map((record) => record.id),
			columns: columns.map((column) => column.key)
		});
		const rows = records.map((record) => ({
			record_id: record.id,
			title: record.title,
			values: Object.fromEntries(
				columns.map((column) => [column.key, String(record.values[column.key] ?? '')])
			)
		}));
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				format: exportFormat,
				file_name: `order_line_records.${exportFormat}`,
				columns,
				rows,
				...(exportFormat === 'csv'
					? { csv: ['title', ...records.map((record) => record.title)].join('\\n') }
					: {})
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/records` && req.method === 'GET') {
		orderLineRecordListRequests.push({
			view_id: url.searchParams.get('view_id'),
			rows: orderLineRecordsForUrl(url).map((record) => record.id)
		});
		json(res, 200, apiResult(paginated(orderLineRecordsForUrl(url))));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/records` && req.method === 'POST') {
		createdRecordPayload = await readBody(req);
		createdRecord = {
			id: 'record-order-line-created',
			workspace_id: workspaceId,
			project_id: projectId,
			form_id: orderLineForm.id,
			title: createdRecordPayload.title || 'Beef Noodles x2',
			values: {
				...createdRecordPayload.values,
				quantity: { type: 'integer', value: 2 },
				unit_price: { type: 'amount', decimal: '9.99', currency: 'CNY', scale: 2 },
				line_total: { type: 'amount', decimal: '19.98', currency: 'CNY', scale: 2 },
				ticket_no: 'AUTO-000001'
			},
			source: { type: 'web' },
			created_by: 'user-smoke',
			updated_by: 'user-smoke',
			created_at: now,
			updated_at: now
		};
		json(res, 200, apiResult(createdRecord));
		return;
	}
	if (pathname.startsWith('/api/v1/form-views/') && req.method === 'PATCH') {
		const viewId = pathname.slice('/api/v1/form-views/'.length);
		const view = orderLineViews.find((item) => item.id === viewId);
		if (!view) {
			json(res, 404, { code: 404, message: 'view not found', data: null });
			return;
		}
		updatedViewPayload = await readBody(req);
		updatedViewPayloads.push(updatedViewPayload);
		if (updatedViewPayload.expected_updated_at !== view.updated_at) {
			json(res, 200, { code: 409, message: 'view update conflict', data: null });
			return;
		}
		if (typeof updatedViewPayload.name === 'string') view.name = updatedViewPayload.name;
		if (typeof updatedViewPayload.view_type === 'string')
			view.view_type = updatedViewPayload.view_type;
		if (updatedViewPayload.config && typeof updatedViewPayload.config === 'object') {
			view.config = updatedViewPayload.config;
			if (view.config.is_default === true) {
				for (const item of orderLineViews) {
					if (item.id !== view.id) item.config = { ...item.config, is_default: false };
				}
			}
		}
		view.updated_at = nextUpdatedAt();
		json(res, 200, apiResult(view));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-comparison' && req.method === 'PATCH') {
		updatedRecordPayload = await readBody(req);
		updatedRecordPayloads.push(updatedRecordPayload);
		if (memberPermissionPolicy.record_scope === 'owned') {
			json(res, 403, {
				code: 403,
				message: 'form record ownership policy denied access',
				data: null
			});
			return;
		}
		comparisonRecord.values = updatedRecordPayload.values || comparisonRecord.values;
		comparisonRecord.source = updatedRecordPayload.source || comparisonRecord.source;
		comparisonRecord.updated_by = 'user-smoke';
		comparisonRecord.updated_at = now;
		json(res, 200, apiResult(comparisonRecord));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created' && req.method === 'GET') {
		json(res, 200, apiResult(seedCreatedRecord()));
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created' && req.method === 'PATCH') {
		updatedRecordPayload = await readBody(req);
		updatedRecordPayloads.push(updatedRecordPayload);
		const lockedField = Object.keys(updatedRecordPayload.values ?? {}).find(
			(fieldKey) => memberPermissionPolicy.fields?.[fieldKey]?.write === false
		);
		if (lockedField) {
			json(res, 403, {
				code: 403,
				message: 'form field write permission denied: ' + lockedField,
				data: null
			});
			return;
		}
		createdRecord = {
			...createdRecord,
			title: updatedRecordPayload.title || createdRecord.title,
			values: updatedRecordPayload.values || createdRecord.values,
			source: updatedRecordPayload.source || createdRecord.source,
			updated_by: 'user-smoke',
			updated_at: now
		};
		json(res, 200, apiResult(createdRecord));
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}/records`) {
		json(res, 200, apiResult(paginated(printJobs)));
		return;
	}
	if (pathname === `/api/v1/forms/${orderLineForm.id}/aggregate`) {
		const fieldKey = url.searchParams.get('field_key');
		const aggregateByField = {
			quantity: {
				field_type: 'integer',
				decimal: createdRecord ? '2' : null,
				currency: null,
				scale: null
			},
			unit_price: {
				field_type: 'amount',
				decimal: createdRecord ? '9.99' : null,
				currency: 'CNY',
				scale: 2
			},
			line_total: {
				field_type: 'amount',
				decimal: createdRecord ? '19.98' : null,
				currency: 'CNY',
				scale: 2
			}
		};
		const aggregate = aggregateByField[fieldKey] ?? {
			field_type: 'number',
			decimal: createdRecord ? '0' : null,
			currency: null,
			scale: null
		};
		json(
			res,
			200,
			apiResult({
				form_id: orderLineForm.id,
				field_key: fieldKey,
				field_type: aggregate.field_type,
				aggregate: 'sum',
				decimal: aggregate.decimal,
				count: createdRecord ? 1 : 0,
				currency: aggregate.currency,
				scale: aggregate.scale
			})
		);
		return;
	}
	if (pathname === `/api/v1/forms/${printJobForm.id}/aggregate`) {
		json(
			res,
			200,
			apiResult({
				form_id: printJobForm.id,
				field_key: url.searchParams.get('field_key'),
				field_type: 'integer',
				aggregate: 'sum',
				decimal: '1',
				count: 1,
				currency: null,
				scale: null
			})
		);
		return;
	}
	if (pathname === '/api/v1/form-records/record-order-line-created/links' && req.method === 'GET') {
		json(
			res,
			200,
			apiResult(
				['detail-widget-empty-text-builder', 'detail-widget-hide-empty-builder'].includes(
					requestedSmokeMode
				)
					? []
					: createdLinks
			)
		);
		return;
	}
	if (
		pathname === '/api/v1/form-records/record-order-line-created/children' &&
		req.method === 'GET'
	) {
		json(
			res,
			200,
			apiResult(
				paginated(
					printJobs.map((record) => ({
						link_id: `link-${record.id}`,
						relation_key: 'line_print_jobs',
						relation_type: 'parent_child',
						record
					}))
				)
			)
		);
		return;
	}
	if (
		pathname === '/api/v1/form-records/record-order-line-created/links' &&
		req.method === 'POST'
	) {
		createdLinkPayload = await readBody(req);
		const link = {
			id: 'link-order-line',
			workspace_id: workspaceId,
			project_id: projectId,
			source_record_id: 'record-order-line-created',
			target_type: createdLinkPayload.target_type,
			target_id: createdLinkPayload.target_id,
			relation_key: createdLinkPayload.relation_key,
			relation_type: createdLinkPayload.relation_type,
			metadata: createdLinkPayload.metadata ?? {},
			created_by: 'user-smoke',
			created_at: now
		};
		createdLinks = [link];
		json(res, 200, apiResult(link));
		return;
	}

	await serveStatic(req, res);
}

if (!existsSync(buildDir)) {
	throw new Error(`Missing build directory: ${buildDir}. Run bun run build first.`);
}

const server = createServer((req, res) => {
	handler(req, res).catch((error) => {
		json(res, 500, { code: 500, message: error.message, data: null });
	});
});
const port = Number(process.env.OPENPR_FORMS_UI_PORT) || 19580 + Math.floor(Math.random() * 1000);
await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));
console.log(`SMOKE_SERVER_READY ${port}`);
await new Promise(() => {});
