function isRecord(data: unknown): data is Record<string, unknown> {
	return typeof data === 'object' && data !== null;
}

export function extractNumber(data: unknown, key: string, fallback: number): number {
	if (!isRecord(data)) {
		return fallback;
	}
	const value = data[key];
	return typeof value === 'number' ? value : fallback;
}

/** A request the viewer cannot read: the object is gone or hidden (403/404, legacy or typed). */
export function isMissingResult(result: { code: number; error_code?: string }): boolean {
	return (
		result.code === 403 ||
		result.code === 404 ||
		result.error_code === 'forbidden' ||
		result.error_code === 'not_found'
	);
}
