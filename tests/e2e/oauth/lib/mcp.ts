// MCP JSON-RPC over POST to the MCP resource (the delegated transport of contract 12).

import { call, type HttpResult } from './http.ts';
import type { Recorder } from './record.ts';
import { asArray, asRecord, str } from './util.ts';

export function mcpRpc(
	rec: Recorder,
	url: string,
	token: string | undefined,
	label: string,
	method: string,
	params?: unknown
): Promise<HttpResult> {
	return call({
		rec,
		label,
		method: 'POST',
		url,
		bearer: token,
		json: { jsonrpc: '2.0', id: 1, method, ...(params === undefined ? {} : { params }) }
	});
}

export function toolNames(res: HttpResult): string[] | null {
	const tools = asArray(asRecord(asRecord(res.json)?.result)?.tools);
	if (tools === null) return null;
	return tools.map((tool) => str(asRecord(tool)?.name) ?? '');
}

export interface ToolOutcome {
	/** HTTP-level success and no JSON-RPC error. */
	rpcError: { code: number | undefined; message: string | undefined } | null;
	isError: boolean;
	text: string;
}

export function toolOutcome(res: HttpResult): ToolOutcome {
	const body = asRecord(res.json);
	const error = asRecord(body?.error);
	const result = asRecord(body?.result);
	const content = asArray(result?.content) ?? [];
	const text = content.map((part) => str(asRecord(part)?.text) ?? '').join('\n');
	const code = error?.code;
	return {
		rpcError:
			error === null
				? null
				: { code: typeof code === 'number' ? code : undefined, message: str(error?.message) },
		isError: result?.isError === true,
		text
	};
}
