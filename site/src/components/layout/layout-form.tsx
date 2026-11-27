import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { isEditableType } from "../../lib/fields/types";
import { isEmptyValue, validateField } from "../../lib/fields/validate";
import { resolveText, resolveValue } from "../../lib/layout/refs";
import { issueField } from "../../lib/site/client";
import { useSite } from "../../lib/site/store";
import type { SiteField } from "../../lib/site/types";
import { FieldInput } from "../field-input";
import { Button } from "../ui";
import { useRuntime } from "./context";

interface FormFieldDef extends SiteField {
  read_only?: boolean;
}

let formCounter = 0;

/** The `form` node: a `form` data source as editors, one `submit` action (contract section 1.4). */
export function LayoutForm(props: { props: Record<string, unknown> }): JSX.Element {
  const runtime = useRuntime();
  const site = useSite();
  const idPrefix = `lf${++formCounter}`;
  const sourceName = () =>
    typeof props.props.data === "string" ? props.props.data.replace(/^\$data\./, "") : "";
  const submitName = () => (typeof props.props.submit === "string" ? props.props.submit : "");
  const action = () => runtime.spec.actions?.[submitName()];
  const mode = (): "create" | "update" => {
    const a = action();
    return a && a.kind === "submit" ? a.mode : "create";
  };
  const bindingKey = () => {
    const spec = runtime.spec.data?.[sourceName()];
    const form = spec && spec.source === "form" ? spec.form : undefined;
    const match = typeof form === "string" ? /^binding:(.+)$/.exec(form) : null;
    return match?.[1] ?? "";
  };

  const fields = createMemo<FormFieldDef[]>(() => {
    const source = runtime.data()[sourceName()] as { fields?: FormFieldDef[] } | undefined;
    const all = source?.fields ?? [];
    const subset = Array.isArray(props.props.fields) ? (props.props.fields as string[]) : null;
    const editable = new Set(runtime.editableFields(bindingKey(), mode()));
    return all.filter(
      (field) =>
        !field.read_only &&
        isEditableType(field.type) &&
        editable.has(field.key) &&
        (!subset || subset.includes(field.key)),
    );
  });

  // Update mode starts from the stored record the action points at.
  const stored = createMemo<Record<string, unknown>>(() => {
    const a = action();
    if (a?.kind !== "submit" || a.mode !== "update") return {};
    const id = resolveValue(a.record, runtime.scope());
    for (const value of Object.values(runtime.data())) {
      const row = value as { id?: unknown; values?: Record<string, unknown> } | undefined;
      if (row && row.id === id && row.values) return row.values;
    }
    return {};
  });

  const [edits, setEdits] = createSignal<Record<string, unknown>>({});
  const [hints, setHints] = createSignal<Record<string, string>>({});
  const currentValue = (key: string) => (key in edits() ? edits()[key] : stored()[key]);

  const state = () => runtime.actions()[submitName()];
  const unavailable = () => runtime.unavailable(submitName());
  const busy = () => state()?.status === "running";
  const issues = createMemo(() => {
    const map: Record<string, string> = {};
    const general: string[] = [];
    for (const issue of state()?.issues ?? []) {
      const key = issueField(issue);
      const text = issue.message ?? issue.code;
      if (key && fields().some((field) => field.key === key)) map[key] = text;
      else general.push(text);
    }
    return { map, general };
  });

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const nextHints: Record<string, string> = {};
    for (const field of fields()) {
      const verdict = validateField(field, currentValue(field.key));
      if (verdict)
        nextHints[field.key] = verdict === "required" ? site.t("form.required") : verdict;
    }
    setHints(() => nextHints);
    if (Object.keys(nextHints).length > 0) return;
    const values: Record<string, unknown> = {};
    for (const field of fields()) {
      const value = currentValue(field.key);
      if (mode() === "update" ? field.key in edits() : !isEmptyValue(value))
        values[field.key] = value;
    }
    if (await runtime.run(submitName(), values)) setEdits(() => ({}));
  };

  const failure = () => {
    const current = state();
    if (current?.status !== "error") return "";
    if (current.error === "conflict") return site.t("form.conflict");
    if (current.error === "http_429" || current.error === "rate_limited")
      return site.t("form.rateLimited");
    if (current.error === "validation_failed" && issues().general.length === 0) return "";
    return issues().general.join(" ") || site.t("form.failed", { code: current.error ?? "error" });
  };

  return (
    <form class="flex min-w-0 flex-col gap-3" data-layout-node="form" onSubmit={submit} novalidate>
      <For each={fields()}>
        {(field) => (
          <FieldInput
            field={field}
            idPrefix={idPrefix}
            value={currentValue(field.key)}
            error={hints()[field.key] ?? issues().map[field.key]}
            onChange={(value) => {
              setEdits((current) => ({ ...current, [field.key]: value }));
              setHints((current) => ({ ...current, [field.key]: "" }));
            }}
          />
        )}
      </For>
      <Show when={failure()}>
        <p class="text-meta text-danger m-0" role="alert">
          {failure()}
        </p>
      </Show>
      <div title={unavailable() ?? undefined}>
        <Button type="submit" disabled={unavailable() !== null || busy()}>
          {props.props.submit_label
            ? resolveText(props.props.submit_label, runtime.scope(), runtime.locale)
            : site.t("layout.submit")}
        </Button>
      </div>
    </form>
  );
}
