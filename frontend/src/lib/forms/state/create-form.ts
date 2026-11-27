import { describeResult } from '#lib/i18n/describe.js';
import { formsApi } from '#lib/api/forms.js';
import { projectsApi, type TemplatePackage } from '#lib/api/projects.js';
import { toast } from '#lib/stores/toast.js';
import { blankFieldDraft } from '#lib/forms/designer-schema.js';
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

	/** Installs a form-template package into the project and opens its first form. */
	installTemplatePackage = async (template: TemplatePackage) => {
		this.s.installingTemplateKey = template.package_key;

		const response = await projectsApi.installTemplate(this.s.projectId, template.package_key);

		this.s.installingTemplateKey = null;

		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response));

			return;
		}

		await this.fp.loaders.loadForms();

		this.s.showCreateForm = false;
		toast.success(translate('forms.templateInstalled'));

		const first = response.data.forms[0];

		if (first) {
			await this.fp.session.openForm(first.form_id);
			this.s.activeMode = 'design';
		}
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
