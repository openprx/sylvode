/**
 * CommonMark subset to HTML for the `markdown` component. All input is escaped first, so no raw
 * HTML survives; links are `https:` or relative only (contract section 1.4); images are dropped.
 */

function escapeHtml(input: string): string {
  return input
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/** `https://...` and `/relative` targets only (never `//host`). */
function safeHref(target: string): string | null {
  const t = target.replaceAll("&amp;", "&");
  if (/^https:\/\/[^\s]+$/i.test(t)) return target;
  if (/^\/(?!\/)[^\s]*$/.test(t)) return target;
  return null;
}

function renderInline(input: string): string {
  let text = escapeHtml(input);
  text = text.replace(/`([^`]+)`/g, "<code>$1</code>");
  text = text.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  text = text.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  text = text.replace(/\[([^\]]+)\]\(([^\s)]+)\)/g, (_all, label: string, target: string) => {
    const href = safeHref(target);
    return href === null
      ? label
      : `<a href="${href}" target="_blank" rel="noopener noreferrer">${label}</a>`;
  });
  return text;
}

export function renderMarkdown(content: string): string {
  const source = content.trim();
  if (!source) return "";
  const out: string[] = [];
  let inCode = false;
  let list: string[] = [];
  const flush = () => {
    if (list.length > 0) out.push(`<ul>${list.join("")}</ul>`);
    list = [];
  };
  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trimEnd();
    if (line.startsWith("```")) {
      flush();
      out.push(inCode ? "</code></pre>" : "<pre><code>");
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      out.push(`${escapeHtml(raw)}\n`);
      continue;
    }
    if (!line.trim()) {
      flush();
      continue;
    }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      flush();
      const level = (heading[1] ?? "#").length;
      out.push(`<h${level}>${renderInline(heading[2] ?? "")}</h${level}>`);
      continue;
    }
    const item = /^[-*]\s+(.+)$/.exec(line);
    if (item) {
      list.push(`<li>${renderInline(item[1] ?? "")}</li>`);
      continue;
    }
    flush();
    out.push(`<p>${renderInline(line)}</p>`);
  }
  flush();
  if (inCode) out.push("</code></pre>");
  return out.join("");
}
