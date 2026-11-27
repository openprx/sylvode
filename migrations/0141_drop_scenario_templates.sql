-- Built-in template packages (low-code programme, L3.T1m) replace the scenario templates: the six
-- system templates are now signed packages under `packages/builtin/` that project creation installs
-- through the installation flow. Safe to run again: every statement is guarded.
--
-- A project created from a template keeps its forms and records; the template description that older
-- versions copied into `projects.type_settings` (`scenario_template`, `scenario_template_key`) stays
-- as an inert historical value and nothing reads it any more.

DROP TABLE IF EXISTS scenario_templates;
