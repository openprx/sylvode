export type PackageWarningObjectType = 'page' | 'navigator' | 'collection' | 'record';
export const PACKAGE_WARNING_KIND_KEYS: Record<PackageWarningObjectType, string> = {
	page: 'flow.package.kindPage',
	navigator: 'flow.package.kindNavigator',
	collection: 'flow.package.kindCollection',
	record: 'flow.package.kindRecord'
};
export interface PackageExportWarning {
	messageKey: string;
	objectTypes: PackageWarningObjectType[];
}
/** Interpret only server-recorded warnings. Never infer omitted counts or complete scope. */
export function packageExportWarnings(raw: unknown, scopeExport = false): PackageExportWarning[] {
	const result: PackageExportWarning[] = [];
	const warnings = Array.isArray(raw)
		? raw.filter((value): value is string => typeof value === 'string')
		: [];
	if (scopeExport && warnings.length === 0)
		return [{ messageKey: 'flow.package.selectionUnrecorded', objectTypes: [] }];
	for (const value of warnings.slice(0, 32)) {
		const parts = value.split(':');
		const [reason, list] = parts;
		const names = list?.split(',') ?? [];
		const known =
			parts.length === 2 &&
			names.length > 0 &&
			names.every((name) => Object.hasOwn(PACKAGE_WARNING_KIND_KEYS, name));
		const types = known
			? (['page', 'navigator', 'collection', 'record'] as const).filter((type) =>
					names.includes(type)
				)
			: [];
		const messageKey =
			known &&
			reason === 'object_type_filter_applied' &&
			types.every((type) => type === 'page' || type === 'navigator')
				? 'flow.package.filterApplied'
				: known && reason === 'object_types_omitted'
					? 'flow.package.typesOmitted'
					: 'flow.package.additionalWarning';
		if (
			!result.some(
				(item) => item.messageKey === messageKey && item.objectTypes.join(',') === types.join(',')
			)
		)
			result.push({
				messageKey,
				objectTypes: messageKey === 'flow.package.additionalWarning' ? [] : types
			});
	}
	return result;
}
