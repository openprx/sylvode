export type TitleTranslate = (key: string, options?: { values?: Record<string, string> }) => string;

interface TitleRule {
	pattern: RegExp;
	key: string;
	values?: Record<string, string>;
}

const rule = (pattern: RegExp, key: string): TitleRule => ({ pattern, key });

/**
 * Document title of each route, as the app layout built it; first match wins. The tail adds the
 * routes of the shell redesign: the project Apps index and the project pages that moved under the
 * workspace (audit reports).
 */
const RULES: readonly TitleRule[] = [
	rule(/^\/plugins(\/|$)/, 'plugins.library.title'),
	rule(/^\/flow\/[^/]+\/convert$/, 'pageTitle.flowConvert'),
	rule(/^\/flow\/conversions\/[^/]+$/, 'pageTitle.flowConversion'),
	rule(/^\/flow(\/|$)/, 'nav.flow'),
	rule(/^\/workspace$/, 'pageTitle.workspace'),
	rule(/^\/governance$/, 'pageTitle.governanceCenter'),
	rule(/^\/proposals$/, 'pageTitle.proposals'),
	rule(/^\/proposals\/new$/, 'pageTitle.proposalNew'),
	rule(/^\/impact-reviews$/, 'pageTitle.impactReviews'),
	rule(/^\/proposal-templates$/, 'pageTitle.proposalTemplates'),
	rule(/^\/decisions\/analytics$/, 'pageTitle.decisionAnalytics'),
	rule(/^\/governance\/config$/, 'pageTitle.governanceConfig'),
	rule(/^\/governance\/audit-logs$/, 'pageTitle.governanceAuditLogs'),
	rule(/^\/proposals\/[^/]+\/review$/, 'pageTitle.proposalImpactReview'),
	rule(/^\/proposals\/[^/]+\/edit$/, 'pageTitle.proposalEdit'),
	rule(/^\/proposals\/[^/]+\/veto$/, 'pageTitle.vetoDetail'),
	rule(/^\/proposals\/[^/]+\/chain$/, 'pageTitle.proposalChain'),
	rule(/^\/proposals\/[^/]+$/, 'pageTitle.proposalDetailFallback'),
	rule(/^\/trust-board$/, 'pageTitle.trustBoard'),
	rule(/^\/trust-scores\/appeals$/, 'pageTitle.trustAppeals'),
	{ pattern: /^\/users\/[^/]+\/trust$/, key: 'pageTitle.userTrust', values: { id: '' } },
	rule(/^\/projects\/[^/]+\/audit-reports\/[^/]+$/, 'pageTitle.auditReportDetail'),
	rule(/^\/projects\/[^/]+\/audit-reports$/, 'pageTitle.auditReports'),
	rule(/^\/search$/, 'pageTitle.search'),
	rule(/^\/inbox$/, 'pageTitle.inbox'),
	rule(/^\/my-work$/, 'pageTitle.myWork'),
	rule(/^\/settings$/, 'pageTitle.personalSettings'),
	rule(/^\/admin$/, 'pageTitle.adminDashboard'),
	rule(/^\/admin\/users$/, 'pageTitle.adminUsers'),
	rule(/^\/admin\/settings$/, 'pageTitle.adminSettings'),
	rule(/^\/workspace\/[^/]+\/settings$/, 'pageTitle.workspaceSettings'),
	rule(/^\/workspace\/[^/]+\/settings\/flow$/, 'pageTitle.flowSettings'),
	rule(/^\/workspace\/[^/]+\/settings\/flow\/package$/, 'pageTitle.flowPackage'),
	rule(
		/^\/workspace\/[^/]+\/settings\/flow\/package\/imports\/[^/]+$/,
		'pageTitle.flowImportReport'
	),
	rule(/^\/workspace\/[^/]+\/settings\/flow\/operations$/, 'pageTitle.flowOperations'),
	rule(/^\/workspace\/[^/]+\/flow\/[^/]+\/convert$/, 'pageTitle.flowConvert'),
	rule(/^\/workspace\/[^/]+\/flow\/conversions\/[^/]+$/, 'pageTitle.flowConversion'),
	rule(/^\/workspace\/[^/]+\/flow(\/[^/]+)?$/, 'pageTitle.flowWorkspace'),
	rule(/^\/workspace\/[^/]+\/members$/, 'pageTitle.workspaceMembers'),
	rule(/^\/workspace\/[^/]+\/webhooks$/, 'pageTitle.workspaceWebhooks'),
	rule(/^\/workspace\/[^/]+\/connections$/, 'pageTitle.workspaceConnections'),
	rule(/^\/workspace\/[^/]+\/workflows$/, 'pageTitle.workspaceWorkflows'),
	rule(/^\/workspace\/[^/]+\/milestones$/, 'milestones.pageTitle.list'),
	rule(/^\/workspace\/[^/]+\/milestones\/[^/]+$/, 'milestones.pageTitle.detail'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/issues\/[^/]+$/, 'pageTitle.issueDetail'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/issues$/, 'pageTitle.issueList'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/board$/, 'pageTitle.projectBoard'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/cycles$/, 'pageTitle.projectCycles'),
	rule(
		/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/plugins\/creator$/,
		'plugins.creator.title'
	),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings(\/plugins)?$/, 'plugins.projectSettings'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/execution$/, 'execution.connection.title'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/inbound-sources$/, 'inbound.sources.title'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/alert-rules$/, 'inbound.rules.title'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/navigation$/, 'shell.sections.navigation'),
	rule(/^\/workspace\/[^/]+\/settings\/navigation$/, 'shell.sections.navigation'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/settings\/drafts$/, 'drafts.title'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/pages\/[^/]+$/, 'nav.page.title'),
	rule(/^\/workspace\/[^/]+\/pages\/[^/]+$/, 'nav.page.title'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/forms$/, 'pageTitle.projectForms'),
	rule(/^\/workspace\/[^/]+\/projects$/, 'pageTitle.workspaceProjects'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+$/, 'pageTitle.projectDetail'),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/apps$/, 'shell.pageTitle.projectApps'),
	rule(
		/^\/workspace\/[^/]+\/projects\/[^/]+\/audit-reports\/[^/]+$/,
		'pageTitle.auditReportDetail'
	),
	rule(/^\/workspace\/[^/]+\/projects\/[^/]+\/audit-reports$/, 'pageTitle.auditReports')
];

const WORKSPACE_HOME = /^\/workspace\/([^/]+)\/?$/;

export interface TitleResult {
	key: string;
	values?: Record<string, string>;
}

/** The message key (and values) of a route's document title. Unknown routes get `pageTitle.appDefault`. */
export function pageTitleKey(
	pathname: string,
	workspaceNames: Readonly<Record<string, string | null | undefined>> = {}
): TitleResult {
	const home = WORKSPACE_HOME.exec(pathname);
	if (home) {
		const name = workspaceNames[home[1]]?.trim();
		return name
			? { key: 'pageTitle.workspaceHomeWithName', values: { name } }
			: { key: 'pageTitle.workspaceHome' };
	}
	for (const r of RULES) if (r.pattern.test(pathname)) return { key: r.key, values: r.values };
	return { key: 'pageTitle.appDefault' };
}

/**
 * Document title of a route. `workspaceNames` are the names the workspace home pages have
 * loaded (`workspaceHomeNames`), so `/workspace/{id}` reads "Sylvode - {name}" without a request.
 */
export function pageTitle(
	pathname: string,
	translate: TitleTranslate,
	workspaceNames: Readonly<Record<string, string | null | undefined>> = {}
): string {
	const { key, values } = pageTitleKey(pathname, workspaceNames);
	return values ? translate(key, { values }) : translate(key);
}
