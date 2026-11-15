import type { FlowCapabilities } from '../api/flow';
/** Rollout capabilities are independent of ordinary personal writes and legacy workspace flags. */
export function personalPackageReadiness(capabilities: Partial<FlowCapabilities> | null) {
	const supported = capabilities?.api_version === 2;
	const imports = supported && capabilities?.personal_package_imports === true;
	const exports =
		supported &&
		capabilities?.personal_scope_package_exports === true &&
		Array.isArray(capabilities.personal_package_export_selections) &&
		capabilities.personal_package_export_selections.includes('scope');
	return {
		imports,
		exports,
		historyImports: imports && capabilities?.personal_package_history_imports === true,
		historyExports: exports && capabilities?.personal_package_history_exports === true
	};
}
