import type { JSX } from "@solidjs/web";
import { createSignal, Errored, For, Match, Show, Switch } from "solid-js";
import { formatValue } from "../../lib/layout/format";
import { renderMarkdown } from "../../lib/layout/markdown";
import { displayValue, resolveText, resolveValue } from "../../lib/layout/refs";
import type { LayoutNode as NodeSpec } from "../../lib/layout/types";
import { evaluateWhen } from "../../lib/layout/when";
import { useSite } from "../../lib/site/store";
import { Button } from "../ui";
import { useRuntime } from "./context";
import { LayoutForm } from "./layout-form";

const GAP: Record<string, string> = {
  none: "gap-0",
  xs: "gap-1",
  sm: "gap-2",
  md: "gap-3",
  lg: "gap-4",
};
const ALIGN: Record<string, string> = {
  start: "items-start",
  center: "items-center",
  end: "items-end",
  stretch: "items-stretch",
};
const COLUMNS: Record<number, string> = {
  1: "grid-cols-1",
  2: "grid-cols-1 sm:grid-cols-2",
  3: "grid-cols-1 sm:grid-cols-2 lg:grid-cols-3",
  4: "grid-cols-1 sm:grid-cols-2 lg:grid-cols-4",
};
/** Static class names so Tailwind sees them; the 2-column breakpoint clamps a wider span to 2. */
const COL_SPAN: Record<number, string> = {
  2: "sm:col-span-2",
  3: "sm:col-span-2 lg:col-span-3",
  4: "sm:col-span-2 lg:col-span-4",
};
const TEXT_STYLE: Record<string, string> = {
  body: "text-small text-text",
  muted: "text-small text-secondary",
  heading: "text-body font-semibold text-text",
  caption: "text-meta text-secondary",
};
const TONE_CLASS: Record<string, string> = {
  neutral: "bg-surface text-text",
  info: "bg-selected text-text",
  success: "bg-success-bg text-success",
  warning: "bg-warning-bg text-warning",
  danger: "bg-danger-bg text-danger",
};
const BUTTON_VARIANT = new Set(["primary", "secondary", "danger", "ghost"]);

export const KNOWN_TYPES = new Set([
  "stack",
  "grid",
  "card",
  "tabs",
  "text",
  "markdown",
  "stat",
  "badge",
  "table",
  "field-display",
  "form",
  "button",
]);

/** Grid children: a child with `col_span` above 1 sits in a wrapper that spans that many columns. */
function GridChildren(props: { nodes: NodeSpec[] | undefined }): JSX.Element {
  return (
    <For each={props.nodes ?? []}>
      {(child) => {
        const span = COL_SPAN[Number(child.col_span)];
        return span ? (
          <div class={`min-w-0 empty:hidden ${span}`} data-layout-span={child.col_span}>
            <LayoutNode node={child} />
          </div>
        ) : (
          <LayoutNode node={child} />
        );
      }}
    </For>
  );
}

function Children(props: { nodes: NodeSpec[] | undefined }): JSX.Element {
  return <For each={props.nodes ?? []}>{(child) => <LayoutNode node={child} />}</For>;
}

/** One node with its own error boundary: a failing node never takes the page down. */
export function LayoutNode(props: { node: NodeSpec }): JSX.Element {
  const site = useSite();
  return (
    <Errored
      fallback={
        <div
          class="rounded-input border border-dashed border-line px-3 py-2 text-meta text-secondary"
          data-layout-error={props.node.type}
        >
          {site.t("layout.nodeError")}
        </div>
      }
    >
      <NodeView node={props.node} />
    </Errored>
  );
}

function NodeView(props: { node: NodeSpec }): JSX.Element {
  const runtime = useRuntime();
  const site = useSite();
  const node = props.node;
  const cfg = node.props ?? {};
  const text = (value: unknown) => resolveText(value, runtime.scope(), runtime.locale);
  const value = (raw: unknown) => resolveValue(raw, runtime.scope());
  const loc = runtime.locale;
  const gap = GAP[String(cfg.gap)] ?? GAP.md ?? "";
  const [collapsed, setCollapsed] = createSignal(false);
  const [activeTab, setActiveTab] = createSignal(0);

  const tabs = () =>
    Array.isArray(cfg.tabs) ? (cfg.tabs as { key?: string; label?: unknown }[]) : [];
  const rows = () => {
    const resolved = value(cfg.rows);
    return Array.isArray(resolved) ? (resolved as Record<string, unknown>[]) : [];
  };
  const columns = () =>
    Array.isArray(cfg.columns)
      ? (cfg.columns as { field: string; label?: unknown; format?: unknown }[]).slice(0, 12)
      : [];
  const cell = (row: Record<string, unknown>, field: string): unknown => {
    const values = row.values;
    if (values && typeof values === "object" && field in (values as object)) {
      return (values as Record<string, unknown>)[field];
    }
    return row[field];
  };
  const badgeTone = () => {
    const shown = displayValue(value(cfg.value));
    const tone = cfg.tone;
    const picked =
      tone && typeof tone === "object" && "map" in tone
        ? (tone as { map: Record<string, string> }).map[shown]
        : tone;
    return typeof picked === "string" && picked in TONE_CLASS ? picked : "neutral";
  };
  const fieldRecord = () => value(cfg.record) as Record<string, unknown> | undefined;
  const fieldKey = typeof cfg.field === "string" ? cfg.field : "";
  const fieldLabel = () => {
    const list = fieldRecord()?.fields;
    const found = Array.isArray(list)
      ? (list as { key: string; label?: string }[]).find((item) => item.key === fieldKey)
      : undefined;
    return found?.label ?? fieldKey;
  };
  const unavailable = () => runtime.unavailable(node.on_click);
  const running = () =>
    node.on_click ? runtime.actions()[node.on_click]?.status === "running" : false;
  const rowClick = typeof cfg.row_click === "string" ? cfg.row_click : null;

  return (
    <Show when={evaluateWhen(node.when, runtime.scope())}>
      <Switch
        fallback={
          <div
            class="rounded-input border border-dashed border-line px-3 py-2 text-meta text-secondary"
            data-layout-placeholder={node.type}
          >
            {site.t("layout.unknownNode", { type: node.type })}
          </div>
        }
      >
        <Match when={node.type === "stack"}>
          <div
            class={`flex min-w-0 ${cfg.direction === "horizontal" ? "flex-row flex-wrap" : "flex-col"} ${gap} ${ALIGN[String(cfg.align)] ?? ""}`}
            data-layout-node="stack"
          >
            <Children nodes={node.children} />
          </div>
        </Match>
        <Match when={node.type === "grid"}>
          <div
            class={`grid min-w-0 ${COLUMNS[Number(cfg.columns)] ?? COLUMNS[2]} ${gap}`}
            data-layout-node="grid"
          >
            <GridChildren nodes={node.children} />
          </div>
        </Match>
        <Match when={node.type === "card"}>
          <section
            class="rounded-card border border-line bg-surface-white p-4"
            data-layout-node="card"
          >
            <Show when={cfg.title || cfg.subtitle}>
              <header class="mb-3 flex items-start justify-between gap-2">
                <div class="min-w-0">
                  <Show when={cfg.title}>
                    <h3 class="m-0 truncate text-body font-semibold text-text">
                      {text(cfg.title)}
                    </h3>
                  </Show>
                  <Show when={cfg.subtitle}>
                    <p class="m-0 text-meta text-secondary">{text(cfg.subtitle)}</p>
                  </Show>
                </div>
                <Show when={cfg.collapsible === true}>
                  <button
                    type="button"
                    class="text-meta text-secondary hover:text-text cursor-pointer bg-transparent border-0"
                    aria-expanded={collapsed() ? "false" : "true"}
                    onClick={() => setCollapsed((current) => !current)}
                  >
                    {collapsed() ? site.t("layout.expand") : site.t("layout.collapse")}
                  </button>
                </Show>
              </header>
            </Show>
            <Show when={!collapsed()}>
              <div class="flex min-w-0 flex-col gap-3">
                <Children nodes={node.children} />
              </div>
            </Show>
          </section>
        </Match>
        <Match when={node.type === "tabs"}>
          <div class="min-w-0" data-layout-node="tabs">
            <div class="mb-3 flex gap-1 border-b border-line" role="tablist">
              <For each={tabs()}>
                {(tab, index) => (
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeTab() === index() ? "true" : "false"}
                    class={`px-3 py-2 text-small cursor-pointer bg-transparent border-0 ${
                      activeTab() === index()
                        ? "border-b-2 border-primary font-medium text-text"
                        : "text-secondary hover:text-text"
                    }`}
                    onClick={() => setActiveTab(index())}
                  >
                    {text(tab.label) || tab.key}
                  </button>
                )}
              </For>
            </div>
            <Show when={node.children?.[activeTab()]}>
              {(child) => (
                <div role="tabpanel">
                  <LayoutNode node={child()} />
                </div>
              )}
            </Show>
          </div>
        </Match>
        <Match when={node.type === "text"}>
          <p
            class={`m-0 min-w-0 break-words ${TEXT_STYLE[String(cfg.style)] ?? TEXT_STYLE.body}`}
            data-layout-node="text"
          >
            {text(cfg.value)}
          </p>
        </Match>
        <Match when={node.type === "markdown"}>
          <div
            class="min-w-0 break-words text-small text-text sv-markdown"
            data-layout-node="markdown"
            innerHTML={renderMarkdown(text(cfg.value))}
          />
        </Match>
        <Match when={node.type === "stat"}>
          <div class="flex flex-col gap-1" data-layout-node="stat">
            <span class="text-meta text-secondary">{text(cfg.label)}</span>
            <span class="text-title font-semibold text-text">
              {formatValue(value(cfg.value), cfg.format, loc)}
            </span>
            <Show when={cfg.trend !== undefined}>
              <span class="text-meta text-secondary">{displayValue(value(cfg.trend))}</span>
            </Show>
          </div>
        </Match>
        <Match when={node.type === "badge"}>
          <span
            class={`inline-flex w-fit items-center rounded-chip px-2 h-5 text-meta font-medium ${TONE_CLASS[badgeTone()]}`}
            data-layout-node="badge"
          >
            {displayValue(value(cfg.value))}
          </span>
        </Match>
        <Match when={node.type === "table"}>
          <div class="min-w-0 overflow-x-auto" data-layout-node="table">
            <Show
              when={rows().length > 0}
              fallback={
                <p class="m-0 text-small text-secondary">
                  {cfg.empty ? text(cfg.empty) : site.t("layout.noRows")}
                </p>
              }
            >
              <table class="w-full border-collapse text-left text-small">
                <thead>
                  <tr class="border-b border-line text-meta text-secondary">
                    <For each={columns()}>
                      {(column) => (
                        <th class="px-2 py-1 font-medium">
                          {column.label ? text(column.label) : column.field}
                        </th>
                      )}
                    </For>
                  </tr>
                </thead>
                <tbody>
                  <For each={rows()}>
                    {(row) => (
                      <tr
                        class={`border-b border-line last:border-0 ${rowClick ? "cursor-pointer hover:bg-hover" : ""}`}
                        onClick={() => rowClick && void runtime.run(rowClick)}
                      >
                        <For each={columns()}>
                          {(column) => (
                            <td class="px-2 py-1 text-text">
                              {formatValue(cell(row, column.field), column.format, loc)}
                            </td>
                          )}
                        </For>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </Show>
          </div>
        </Match>
        <Match when={node.type === "field-display"}>
          <div class="min-w-0 text-small" data-layout-node="field-display">
            <Show when={cfg.label !== false}>
              <span class="text-meta text-secondary">{fieldLabel()}</span>
            </Show>
            <div class="break-words text-text">
              {displayValue(
                (fieldRecord()?.values as Record<string, unknown> | undefined)?.[fieldKey],
              )}
            </div>
          </div>
        </Match>
        <Match when={node.type === "form"}>
          <LayoutForm props={cfg} />
        </Match>
        <Match when={node.type === "button"}>
          <span title={unavailable() ?? undefined} data-layout-node="button">
            <Button
              variant={
                typeof cfg.variant === "string" && BUTTON_VARIANT.has(cfg.variant)
                  ? (cfg.variant as "primary" | "secondary" | "danger" | "ghost")
                  : "secondary"
              }
              disabled={unavailable() !== null || running()}
              onClick={() => node.on_click && void runtime.run(node.on_click)}
            >
              {text(cfg.label)}
            </Button>
          </span>
        </Match>
      </Switch>
    </Show>
  );
}
