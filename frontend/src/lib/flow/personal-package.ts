import type { FlowCapabilities } from '../api/flow';
/** Rollout capabilities are independent of ordinary personal writes and workspace flags. */
export function personalPackageReadiness(capabilities: Partial<FlowCapabilities> | null) {
	return {
		imports: capabilities?.personal_package_imports === true,
		exports: capabilities?.personal_scope_package_exports === true
	};
}
