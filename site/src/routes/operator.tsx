import { Dialog } from "@kobalte/core/dialog";
import { useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createSignal, For, onSettled, Show } from "solid-js";
import { FieldInput } from "../components/field-input";
import { FieldValue } from "../components/field-value";
import { RecordTable } from "../components/record-table";
import { Button } from "../components/ui";
import { ApiError } from "../lib/api";
import { isBlockedType, isEditableType } from "../lib/fields/types";
import { validateField } from "../lib/fields/validate";
import { fieldIssues, issueField } from "../lib/site/client";
import { ManifestGate, useSite } from "../lib/site/store";
import type { Manifest, Paginated, SiteField, SiteRecord } from "../lib/site/types";

const PER_PAGE = 20;

function Operator(props: { binding: string; manifest: Manifest }): JSX.Element {
  const site = useSite();
  const info = props.manifest.bindings.find((item) => item.key === props.binding);
  const readKeys = new Set(info?.operator?.read_fields ?? []);
  const updateKeys = info?.operator?.update_fields ?? [];

  const [fields, setFields] = createSignal<SiteField[]>([]);
  const [result, setResult] = createSignal<Paginated<SiteRecord> | null>(null);
  const [page, setPage] = createSignal(1);
  const [sort, setSort] = createSignal<string | undefined>(undefined);
  const [filter, setFilter] = createSignal<{ field: string; value: string } | null>(null);
  const [message, setMessage] = createSignal("");
  const [editing, setEditing] = createSignal<SiteRecord | null>(null);
  const [edits, setEdits] = createSignal<Record<string, unknown>>({});
  const [issues, setIssues] = createSignal<Record<string, string>>({});
  const [saveError, setSaveError] = createSignal("");
  const [saving, setSaving] = createSignal(false);

  const filterField = () => fields().find((field) => field.type === "single_select");

  const load = async (next = page()) => {
    const current = filter();
    try {
      const data = await site.client.listRecords(props.binding, {
        page: next,
        per_page: PER_PAGE,
        ...(sort() ? { sort: sort() as string } : {}),
        ...(current?.value ? { filter: { [current.field]: current.value } } : {}),
      });
      setResult(() => data);
      setPage(next);
      setMessage("");
    } catch (error) {
      setMessage(
        error instanceof ApiError && error.errorCode === "field_not_filterable"
          ? error.message
          : site.t("app.error"),
      );
    }
  };

  onSettled(() => {
    if (!info?.operator) return;
    void site.client
      .fields(props.binding)
      .then((response) => setFields(() => response.fields.filter((f) => readKeys.has(f.key))))
      .catch(() => setMessage(site.t("app.error")));
    void load(1);
  });

  const toggleSort = (field: string) => {
    setSort((current) => (current === field ? `-${field}` : field));
    void load(1);
  };

  const open = (record: SiteRecord) => {
    setEditing(() => record);
    setEdits(() => ({}));
    setIssues(() => ({}));
    setSaveError("");
  };

  const editable = () =>
    fields().filter((field) => updateKeys.includes(field.key) && isEditableType(field.type));
  const readOnly = () =>
    fields().filter((field) => !updateKeys.includes(field.key) && !isBlockedType(field.type));
  const currentValue = (key: string) => {
    const changed = edits();
    return key in changed ? changed[key] : editing()?.values[key];
  };

  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    const record = editing();
    if (!record) return;
    const hints: Record<string, string> = {};
    for (const field of editable()) {
      if (!(field.key in edits())) continue;
      const verdict = validateField(field, edits()[field.key]);
      if (verdict) hints[field.key] = verdict === "required" ? site.t("form.required") : verdict;
    }
    setIssues(() => hints);
    if (Object.keys(hints).length > 0) return;
    setSaving(true);
    setSaveError("");
    try {
      await site.client.update(props.binding, record.id, edits(), record.updated_at);
      setEditing(() => null);
      await load();
    } catch (error) {
      const next: Record<string, string> = {};
      for (const issue of fieldIssues(error)) {
        const key = issueField(issue);
        if (key) next[key] = issue.message ?? issue.code;
      }
      setIssues(() => next);
      if (error instanceof ApiError && (error.status === 409 || error.status === 412)) {
        setSaveError(site.t("form.conflict"));
      } else if (Object.keys(next).length === 0) {
        setSaveError(
          site.t("form.failed", {
            code: error instanceof ApiError ? (error.errorCode ?? error.status) : "error",
          }),
        );
      }
    } finally {
      setSaving(false);
    }
  };

  const pages = () => Math.max(Math.ceil((result()?.total ?? 0) / PER_PAGE), 1);

  return (
    <section class="flex flex-col gap-4" data-view="operator">
      <h1 class="text-page font-semibold m-0">{site.t("op.title")}</h1>
      <Show
        when={info?.operator}
        fallback={<p class="text-secondary m-0">{site.t("op.noAccess")}</p>}
      >
        <Show when={updateKeys.length === 0}>
          <p class="text-secondary m-0">{site.t("op.readOnly")}</p>
        </Show>
        <Show when={filterField()}>
          {(select) => (
            <label class="flex items-center gap-2 text-secondary">
              {select().label}
              <select
                class="bg-input text-text text-body border border-input-border rounded-input px-3 h-9"
                value={filter()?.value ?? ""}
                onChange={(event) => {
                  setFilter(() => ({ field: select().key, value: event.currentTarget.value }));
                  void load(1);
                }}
              >
                <option value="">{site.t("op.all")}</option>
                <For each={select().options ?? []}>
                  {(option) => <option value={option}>{option}</option>}
                </For>
              </select>
            </label>
          )}
        </Show>
        <Show when={message()}>
          <p class="text-danger m-0" role="alert">
            {message()}
          </p>
        </Show>
        <RecordTable
          fields={fields()}
          rows={result()?.items ?? []}
          locale={site.locale}
          empty={site.t("op.empty")}
          sort={sort()}
          onSort={toggleSort}
          {...(updateKeys.length > 0 ? { onRowClick: open } : {})}
        />
        <Show when={pages() > 1}>
          <div class="flex items-center gap-3">
            <Button
              variant="secondary"
              size="sm"
              disabled={page() <= 1}
              onClick={() => void load(page() - 1)}
            >
              {site.t("op.prev")}
            </Button>
            <span class="text-secondary text-small">
              {site.t("op.page", { page: page(), pages: pages() })}
            </span>
            <Button
              variant="secondary"
              size="sm"
              disabled={page() >= pages()}
              onClick={() => void load(page() + 1)}
            >
              {site.t("op.next")}
            </Button>
          </div>
        </Show>
      </Show>
      <Dialog open={editing() !== null} onOpenChange={(next) => !next && setEditing(() => null)}>
        <Dialog.Portal>
          <Dialog.Overlay class="fixed inset-0 z-40 bg-scrim" />
          <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
            <Dialog.Content class="w-full max-w-md bg-overlay border border-overlay-border rounded-dialog shadow-modal p-6 flex flex-col gap-4">
              <Dialog.Title class="text-subheading font-semibold m-0">
                {editing()?.title ?? site.t("op.edit")}
              </Dialog.Title>
              <form
                class="flex flex-col gap-3"
                onSubmit={save}
                novalidate
                data-view="operator-edit"
              >
                <For each={readOnly()}>
                  {(field) => (
                    <div class="flex flex-col gap-1">
                      <span class="text-secondary font-medium">{field.label}</span>
                      <FieldValue
                        type={field.type}
                        value={editing()?.values[field.key]}
                        locale={site.locale}
                      />
                    </div>
                  )}
                </For>
                <For each={editable()}>
                  {(field) => (
                    <FieldInput
                      field={field}
                      idPrefix="op"
                      value={currentValue(field.key)}
                      error={issues()[field.key]}
                      onChange={(value) =>
                        setEdits((current) => ({ ...current, [field.key]: value }))
                      }
                    />
                  )}
                </For>
                <Show when={saveError()}>
                  <p class="text-meta text-danger m-0" role="alert">
                    {saveError()}
                  </p>
                </Show>
                <div class="flex justify-end gap-2">
                  <Button variant="secondary" type="button" onClick={() => setEditing(() => null)}>
                    {site.t("op.close")}
                  </Button>
                  <Button type="submit" disabled={saving()}>
                    {site.t("op.save")}
                  </Button>
                </div>
              </form>
            </Dialog.Content>
          </div>
        </Dialog.Portal>
      </Dialog>
    </section>
  );
}

/** Operator list and update view of one binding (`scope: all`, update fields of the projection). */
export default function OperatorRoute(): JSX.Element {
  const params = useParams();
  return (
    <ManifestGate>
      {(manifest) => (
        <Show when={params.binding} keyed>
          {(binding) => <Operator binding={binding} manifest={manifest} />}
        </Show>
      )}
    </ManifestGate>
  );
}
