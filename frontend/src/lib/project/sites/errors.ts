import type { ApiResult } from '#lib/api/client.js';
import { describeResult } from '#lib/i18n/describe.js';
import { knownError } from './view';

type Translate = (key: string) => string;

/** The words for a failed call: the console's own text for a known code, else the server's. */
export function errorText(result: ApiResult<unknown>, translate: Translate): string {
	const code = knownError(result.error_code);
	if (code) return translate(`sites.errors.${code}`);
	return describeResult(result);
}
