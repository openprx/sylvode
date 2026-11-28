/**
 * Fixture JSON in the exact shapes of contract C3 section 6.6, for the mock mode
 * (`VITE_SV_SITE_MOCK=1`). Never imported by the production entry path.
 */

import type { LayoutSpec } from "../../layout/types";
import type {
  FieldsResponse,
  ManifestBinding,
  ManifestPage,
  SiteField,
  SiteRecord,
} from "../types";

export const MOCK_SLUG = "demo";
/** Operator credentials of the mock backend only; they exist nowhere else. */
export const MOCK_OPERATOR = {
  email: "operator@demo.test",
  password: "mock-operator",
  user_id: "00000000-0000-4000-8000-0000000000a1",
  display_name: "Demo Operator",
  role: "operator" as const,
};

export const SITE = {
  slug: MOCK_SLUG,
  title: { en: "Help desk", zh: "服务台" },
  theme: "light",
};

export const BINDING: ManifestBinding = {
  key: "requests",
  visitor: {
    read_fields: ["title", "status", "created_at"],
    submit_fields: ["title", "description", "contact_phone", "urgency", "topics"],
  },
  operator: {
    read_fields: ["title", "status", "description", "contact_phone", "urgency", "topics"],
    update_fields: ["status"],
  },
};

const FIELDS: SiteField[] = [
  {
    key: "title",
    label: "Title",
    type: "text",
    required: true,
    rules_client: [{ rule: "max_length", value: 80 }],
  },
  { key: "description", label: "Description", type: "textarea" },
  { key: "contact_phone", label: "Contact phone", type: "phone" },
  { key: "urgency", label: "Urgency", type: "single_select", options: ["low", "normal", "high"] },
  { key: "topics", label: "Topics", type: "multi_select", options: ["billing", "access", "other"] },
  {
    key: "status",
    label: "Status",
    type: "single_select",
    options: ["open", "in_progress", "closed"],
  },
  { key: "created_at", label: "Created", type: "datetime" },
];

export const FIELDS_RESPONSE: FieldsResponse = { fields: FIELDS };

const ISO = "2026-11-20T09:00:00Z";
export function seedRecords(): SiteRecord[] {
  return [
    {
      id: "11111111-1111-4111-8111-111111111111",
      title: "Printer offline",
      values: {
        title: "Printer offline",
        status: "open",
        description: "The second floor printer does not respond.",
        contact_phone: "555-0101",
        urgency: "high",
        topics: ["access"],
        created_at: ISO,
      },
      created_at: ISO,
      updated_at: ISO,
    },
    {
      id: "22222222-2222-4222-8222-222222222222",
      title: "Invoice question",
      values: {
        title: "Invoice question",
        status: "in_progress",
        description: "Which VAT rate applies to the March invoice?",
        contact_phone: "555-0102",
        urgency: "normal",
        topics: ["billing"],
        created_at: "2026-11-21T10:30:00Z",
      },
      created_at: "2026-11-21T10:30:00Z",
      updated_at: "2026-11-21T10:30:00Z",
    },
  ];
}

const HOME: LayoutSpec = {
  schema: "sv.layout.v1",
  key: "demo.home",
  title: { en: "Help desk", zh: "服务台" },
  data: {
    mine: {
      source: "query",
      form: "binding:requests",
      sort: [{ field: "created_at", dir: "desc" }],
      fields: ["title", "status", "created_at"],
      page_size: 10,
    },
  },
  actions: { go_new: { kind: "navigate", to: { route: "/requests/new" } } },
  root: {
    type: "stack",
    props: { gap: "md" },
    children: [
      {
        type: "text",
        props: { value: { en: "How can we help?", zh: "需要什么帮助?" }, style: "heading" },
      },
      {
        type: "markdown",
        props: {
          value:
            "Send us a request and follow it here. See the [showcase](/showcase) for every block.",
        },
      },
      {
        type: "button",
        props: { label: { en: "New request", zh: "新建请求" }, variant: "primary" },
        on_click: "go_new",
      },
      {
        type: "card",
        props: { title: { en: "My requests", zh: "我的请求" } },
        children: [
          {
            type: "table",
            props: {
              rows: "$data.mine.items",
              columns: [
                { field: "title" },
                { field: "status" },
                { field: "created_at", format: "datetime" },
              ],
              empty: { en: "Nothing yet", zh: "暂无" },
            },
          },
        ],
      },
    ],
  },
};

const NEW_REQUEST: LayoutSpec = {
  schema: "sv.layout.v1",
  key: "demo.request.new",
  data: {
    form: {
      source: "form",
      form: "binding:requests",
      fields: ["title", "description", "contact_phone", "urgency", "topics"],
    },
  },
  actions: {
    send: {
      kind: "submit",
      data: "form",
      mode: "create",
      confirm: { en: "Send this request?", zh: "发送此请求?" },
    },
  },
  root: {
    type: "card",
    props: { title: { en: "New request", zh: "新建请求" } },
    children: [
      {
        type: "form",
        props: { data: "$data.form", submit: "send", submit_label: { en: "Send", zh: "发送" } },
      },
    ],
  },
};

const SHOWCASE: LayoutSpec = {
  schema: "sv.layout.v1",
  key: "demo.showcase",
  data: {
    req: {
      source: "record",
      form: "binding:requests",
      record: "11111111-1111-4111-8111-111111111111",
      fields: ["title", "status", "urgency"],
    },
    list: { source: "query", form: "binding:requests", fields: ["title", "status"], page_size: 5 },
  },
  root: {
    type: "stack",
    props: { gap: "md" },
    children: [
      { type: "text", props: { value: "Heading text", style: "heading" } },
      { type: "text", props: { value: "Muted text", style: "muted" } },
      { type: "text", props: { value: "Caption text", style: "caption" } },
      {
        type: "markdown",
        props: {
          value:
            "# Title\n\n- one\n- **two** and `code`\n\nA [link](https://example.com) and <b>no html</b>.",
        },
      },
      {
        type: "grid",
        props: { columns: 3, gap: "md" },
        children: [
          { type: "stat", props: { label: "Total", value: "$data.list.total", format: "number" } },
          { type: "stat", props: { label: "Amount", value: "1234.5", format: "amount" } },
          { type: "stat", props: { label: "Share", value: "42", format: "percent", trend: "up" } },
        ],
      },
      {
        type: "stack",
        props: { direction: "horizontal", gap: "sm", align: "center" },
        children: [
          { type: "badge", props: { value: "ok", tone: "success" } },
          { type: "badge", props: { value: "late", tone: "warning" } },
          { type: "badge", props: { value: "open", tone: { map: { open: "danger" } } } },
          { type: "badge", props: { value: "plain" } },
          { type: "button", props: { label: "Primary", variant: "primary" } },
          { type: "button", props: { label: "Danger", variant: "danger" } },
          { type: "button", props: { label: "Ghost", variant: "ghost" } },
        ],
      },
      {
        type: "card",
        props: { title: "Tabs", subtitle: "two tabs", collapsible: true },
        children: [
          {
            type: "tabs",
            props: {
              tabs: [
                { key: "a", label: "First" },
                { key: "b", label: "Second" },
              ],
            },
            children: [
              { type: "text", props: { value: "First tab body" } },
              { type: "text", props: { value: "Second tab body" } },
            ],
          },
        ],
      },
      { type: "field-display", props: { record: "$data.req", field: "title" } },
      { type: "field-display", props: { record: "$data.req", field: "status", label: false } },
      {
        type: "table",
        props: {
          rows: "$data.list.items",
          columns: [{ field: "title", label: "Title" }, { field: "status" }],
        },
      },
      { type: "text", when: "viewer.role == 'nobody'", props: { value: "hidden by when" } },
      { type: "text", props: { value: { cel: "'computed: ' + string(1 + 2)" } } },
      { type: "holo-chart", props: {} },
    ],
  },
};

const OPERATOR_DETAIL: LayoutSpec = {
  schema: "sv.layout.v1",
  key: "demo.request.detail",
  data: {
    req: {
      source: "record",
      form: "binding:requests",
      record: "$params.id",
      fields: ["title", "status", "description", "contact_phone"],
    },
    form: { source: "form", form: "binding:requests", fields: ["status"] },
  },
  params: { id: { type: "string", required: true } },
  actions: { save: { kind: "submit", data: "form", mode: "update", record: "$params.id" } },
  root: {
    type: "stack",
    props: { gap: "md" },
    children: [
      { type: "field-display", props: { record: "$data.req", field: "title" } },
      { type: "field-display", props: { record: "$data.req", field: "description" } },
      { type: "field-display", props: { record: "$data.req", field: "contact_phone" } },
      {
        type: "badge",
        props: {
          value: "$data.req.values.status",
          tone: { map: { closed: "success", open: "warning" } },
        },
      },
      { type: "form", props: { data: "$data.form", submit: "save", submit_label: "Save" } },
    ],
  },
};

export interface MockPage {
  page: ManifestPage;
  layout: LayoutSpec;
}

export const PAGES: MockPage[] = [
  {
    page: {
      route: "/",
      audience: "visitor",
      nav: { label: { en: "Home", zh: "首页" }, position: 0 },
      layout_key: "demo.home",
    },
    layout: HOME,
  },
  {
    page: {
      route: "/requests/new",
      audience: "visitor",
      nav: { label: { en: "New request", zh: "新建请求" }, position: 1 },
      layout_key: "demo.request.new",
    },
    layout: NEW_REQUEST,
  },
  {
    page: { route: "/showcase", audience: "visitor", nav: null, layout_key: "demo.showcase" },
    layout: SHOWCASE,
  },
  {
    page: {
      route: "/requests/:id",
      audience: "operator",
      nav: null,
      layout_key: "demo.request.detail",
    },
    layout: OPERATOR_DETAIL,
  },
];
