// One HTTP call, recorded with its secrets redacted.

import { redactor, truncate } from './redact.ts';
import type { Recorder } from './record.ts';
import { mono } from './util.ts';

export interface HttpResult {
	status: number;
	headers: Headers;
	text: string;
	/** The parsed JSON body, or null when the body is empty or not JSON. */
	json: unknown;
	/** Monotonic time at which the whole response had been received. */
	doneAt: number;
}

export interface CallSpec {
	rec: Recorder;
	label: string;
	method: string;
	url: string;
	bearer?: string;
	json?: unknown;
	form?: Record<string, string>;
	headers?: Record<string, string>;
	timeoutMs?: number;
}

/** Response headers worth keeping as evidence. Authorization-like headers are never kept. */
const EVIDENCE_HEADERS = [
	'www-authenticate',
	'cache-control',
	'pragma',
	'retry-after',
	'content-type',
	'location',
	'x-accel-buffering',
	'x-content-type-options'
];

export function evidenceHeaders(headers: Headers): Record<string, string> {
	const out: Record<string, string> = {};
	for (const name of EVIDENCE_HEADERS) {
		const value = headers.get(name);
		if (value !== null) out[name] = redactor.text(value);
	}
	return out;
}

export async function call(spec: CallSpec): Promise<HttpResult> {
	const headers: Record<string, string> = { accept: 'application/json', ...(spec.headers ?? {}) };
	let body: string | undefined;
	let requestBody: string | undefined;
	if (spec.bearer !== undefined) headers.authorization = `Bearer ${spec.bearer}`;
	if (spec.json !== undefined) {
		body = JSON.stringify(spec.json);
		headers['content-type'] = 'application/json';
		requestBody = redactor.body(body, 'application/json');
	} else if (spec.form !== undefined) {
		body = new URLSearchParams(spec.form).toString();
		headers['content-type'] = 'application/x-www-form-urlencoded';
		requestBody = redactor.form(body);
	}
	const started = mono();
	let response: Response;
	try {
		response = await fetch(spec.url, {
			method: spec.method,
			headers,
			body,
			redirect: 'manual',
			signal: AbortSignal.timeout(spec.timeoutMs ?? 30_000)
		});
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		spec.rec.add({
			label: spec.label,
			method: spec.method,
			url: redactor.url(spec.url),
			requestBody,
			status: 0,
			headers: {},
			body: redactor.text(`transport error: ${message}`),
			elapsedMs: Math.round(mono() - started)
		});
		throw new Error(redactor.text(`${spec.label}: transport error: ${message}`));
	}
	const text = await response.text();
	const doneAt = mono();
	let json: unknown = null;
	if (text.trim() !== '') {
		try {
			json = JSON.parse(text) as unknown;
		} catch {
			json = null;
		}
	}
	spec.rec.add({
		label: spec.label,
		method: spec.method,
		url: redactor.url(spec.url),
		requestBody,
		status: response.status,
		headers: evidenceHeaders(response.headers),
		body: truncate(redactor.body(text, response.headers.get('content-type')), 2000),
		elapsedMs: Math.round(doneAt - started)
	});
	return { status: response.status, headers: response.headers, text, json, doneAt };
}
