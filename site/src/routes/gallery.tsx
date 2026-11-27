import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Input,
  Modal,
  Select,
  showToast,
  Table,
  TBody,
  TD,
  Textarea,
  TH,
  THead,
  ThemeToggle,
  TR,
} from "../components/ui";

const options = [
  { value: "draft", label: "Draft" },
  { value: "review", label: "In review" },
  { value: "live", label: "Published" },
];

function Specimens(): JSX.Element {
  const [choice, setChoice] = createSignal<(typeof options)[number] | null>(options[0] ?? null);
  return (
    <div class="flex flex-col gap-6 bg-bg text-text p-6 rounded-lg border border-line">
      <Card title="Buttons" description="Variants and sizes.">
        <div class="flex flex-wrap items-center gap-3">
          <Button>Primary</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button size="sm">Small</Button>
          <Button disabled>Disabled</Button>
          <ThemeToggle />
        </div>
      </Card>
      <Card title="Form controls">
        <div class="grid gap-4 sm:grid-cols-2">
          <Input label="Title" placeholder="Page title" description="Shown in the browser tab." />
          <Input label="Slug" defaultValue="bad slug!" errorMessage="Use lowercase letters only." />
          <Textarea label="Summary" placeholder="A short summary" />
          <div class="flex flex-col gap-4">
            <Select
              label="State"
              options={options}
              value={choice() ?? undefined}
              onChange={setChoice}
            />
            <Checkbox label="Show in navigation" defaultChecked />
            <Checkbox label="Disabled option" disabled />
          </div>
        </div>
      </Card>
      <Card title="Badges">
        <div class="flex flex-wrap gap-2">
          <Badge>Neutral</Badge>
          <Badge tone="success">Success</Badge>
          <Badge tone="warning">Warning</Badge>
          <Badge tone="danger">Danger</Badge>
          <Badge tone="solid">Solid</Badge>
        </div>
      </Card>
      <Card title="Table">
        <Table>
          <THead>
            <TR>
              <TH>Page</TH>
              <TH>State</TH>
              <TH class="text-right">Views</TH>
            </TR>
          </THead>
          <TBody>
            <TR>
              <TD>Home</TD>
              <TD>
                <Badge tone="success">Published</Badge>
              </TD>
              <TD class="text-right">1,204</TD>
            </TR>
            <TR>
              <TD>Pricing</TD>
              <TD>
                <Badge tone="warning">In review</Badge>
              </TD>
              <TD class="text-right">87</TD>
            </TR>
          </TBody>
        </Table>
      </Card>
      <Card title="Modal and toast">
        <div class="flex flex-wrap gap-3">
          <Modal
            trigger={<Button variant="secondary">Open modal</Button>}
            title="Delete page"
            description="This cannot be undone."
            footer={<Button variant="danger">Delete</Button>}
          >
            <p class="m-0">The page and its revisions will be removed.</p>
          </Modal>
          <Button
            variant="secondary"
            onClick={() => showToast({ title: "Saved", description: "Changes were stored." })}
          >
            Show toast
          </Button>
        </div>
      </Card>
    </div>
  );
}

export default function Gallery(): JSX.Element {
  return (
    <div class="flex flex-col gap-6">
      <h1 class="text-page font-semibold m-0">Component gallery</h1>
      <h2 class="text-title font-semibold m-0">Light</h2>
      <div data-theme="light" data-gallery="light">
        <Specimens />
      </div>
      <h2 class="text-title font-semibold m-0">Dark</h2>
      <div data-theme="dark" data-gallery="dark">
        <Specimens />
      </div>
    </div>
  );
}
