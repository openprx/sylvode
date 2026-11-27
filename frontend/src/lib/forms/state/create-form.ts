import { describeResult } from '#lib/i18n/describe.js';
import { formsApi } from '#lib/api/forms.js';
import type { ScenarioTemplate } from '#lib/api/projects.js';
import { toast } from '#lib/stores/toast.js';
import { scenarioTemplateDescription, scenarioTemplateName } from '#lib/utils/project-type-i18n.js';
import { blankFieldDraft } from '#lib/forms/designer-schema.js';
import {
	fieldDraftsFromSchema,
	normalizeTemplateSchema,
	templateFormKey,
	templateTitleTemplate
} from '#lib/forms/designer-schema.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { FORM_SCHEMA_VERSION } from '#lib/forms/types/index.js';

export class CreateFormController extends FormsController {
	handleCreateForm = async () => {
		let schema: Record<string, unknown>;

		try {
			const parsed = this.s.showAdvancedSchema
				? (JSON.parse(this.s.formDraft.schemaText) as unknown)
				: this.fp.designer.buildSchemaFromDrafts();

			if (!isRecord(parsed)) throw new Error(translate('forms.schemaMustBeObject'));

			schema = parsed;
		} catch (error) {
			toast.error(error instanceof Error ? error.message : translate('forms.invalidSchema'));

			return;
		}

		this.s.formSubmitting = true;

		const response = await formsApi.create(this.s.projectId, {
			key: this.s.formDraft.key.trim(),
			name: this.s.formDraft.name.trim(),
			description: this.s.formDraft.description.trim(),
			title_template: this.s.formDraft.title_template.trim() || '{id}',
			schema
		});

		this.s.formSubmitting = false;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.forms = [response.data, ...this.s.forms];
		this.s.showCreateForm = false;
		this.resetFormDraft();
		toast.success(translate('forms.formCreated'));
		await this.fp.session.openForm(response.data.id);
		this.s.activeMode = 'design';
		void this.fp.designer.loadDesignerSafety(response.data);
	};

	resetFormDraft = () => {
		this.s.formDraft = {
			key: '',
			name: '',
			description: '',
			title_template: '{id}',
			schemaText: JSON.stringify({ version: FORM_SCHEMA_VERSION, fields: [] }, null, 2)
		};

		this.s.fieldDrafts = [blankFieldDraft()];
		this.s.showAdvancedSchema = false;
	};

	applyScenarioTemplate = (template: ScenarioTemplate) => {
		const schema = normalizeTemplateSchema(template.field_schema);

		this.s.formDraft = {
			key: templateFormKey(template),
			name: scenarioTemplateName(template, translate),
			description: scenarioTemplateDescription(template, translate),
			title_template: templateTitleTemplate(schema),
			schemaText: JSON.stringify(schema, null, 2)
		};

		this.s.fieldDrafts = fieldDraftsFromSchema(schema);
		this.s.showAdvancedSchema = false;
		toast.success(translate('forms.templateApplied'));
	};

	uniqueTemplateKey = (template: ScenarioTemplate): string => {
		const base = templateFormKey(template);

		if (!this.s.forms.some((form) => form.key === base)) return base;

		return `${base}_${Date.now().toString(36)}`;
	};

	installScenarioTemplate = async (template: ScenarioTemplate) => {
		this.s.installingTemplateKey = template.key;

		const schema = normalizeTemplateSchema(template.field_schema);

		const response = await formsApi.createFromTemplate(this.s.projectId, {
			template_key: template.key,
			key: this.uniqueTemplateKey(template),
			name: scenarioTemplateName(template, translate),
			description: scenarioTemplateDescription(template, translate),
			title_template: templateTitleTemplate(schema)
		});

		this.s.installingTemplateKey = null;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		this.s.forms = [response.data, ...this.s.forms.filter((form) => form.id !== response.data?.id)];

		this.s.showCreateForm = false;
		toast.success(translate('forms.templateInstalled'));
		await this.fp.session.openForm(response.data.id);
		this.s.activeMode = 'design';
		void this.fp.designer.loadDesignerSafety(response.data);
	};

	addFieldDraft = () => {
		this.s.fieldDrafts = [...this.s.fieldDrafts, blankFieldDraft()];
	};

	removeFieldDraft = (index: number) => {
		this.s.fieldDrafts =
			this.s.fieldDrafts.length > 1
				? this.s.fieldDrafts.filter((_, itemIndex) => itemIndex !== index)
				: [blankFieldDraft()];
	};
}
