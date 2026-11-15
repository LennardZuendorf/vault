import { openUrl } from "@tauri-apps/plugin-opener";
import { useMemo, type MouseEvent, type ReactNode } from "react";
import { splitKeys } from "../lib/issue-keys";
import { parse, safeHref, type Block, type Inline } from "../lib/markdown";
import { IssueKeyLink } from "./IssueKeyLink";

/* oxlint-disable react/no-array-index-key -- nodes are parsed afresh from the text, with nothing else to tell them apart */

/**
 * Markdown as GitHub shows it, built from React elements only (see
 * `lib/markdown.ts` for why never HTML). Links open in the browser, and a
 * click on one does not reach whatever row the text sits in. Given `keys`,
 * the projects `useKeyProjects` knows, a ticket's key is a link too (ACP-18).
 */
export function Markdown({ text, keys = "" }: { text: string; keys?: string }) {
  const tree = useMemo(() => parse(text), [text]);
  return <div className="md">{tree.map((b, i) => block(b, i, keys))}</div>;
}

function open(e: MouseEvent, href: string) {
  e.preventDefault();
  e.stopPropagation();
  if (safeHref(href)) void openUrl(href).catch(() => {});
}

function inlines(nodes: Inline[], keys: string): ReactNode[] {
  return nodes.map((n, i) => {
    switch (n.t) {
      case "text":
        return keys ? withKeys(n.v, keys, i) : n.v;
      case "code":
        return <code key={i}>{n.v}</code>;
      case "strong":
        return <strong key={i}>{inlines(n.c, keys)}</strong>;
      case "em":
        return <em key={i}>{inlines(n.c, keys)}</em>;
      case "del":
        return <del key={i}>{inlines(n.c, keys)}</del>;
      case "br":
        return <br key={i} />;
      case "link":
        return (
          <a
            key={i}
            className="ui-link"
            href={n.href}
            title={n.href}
            onClick={(e) => open(e, n.href)}
          >
            {inlines(n.c, "")}
          </a>
        );
    }
  });
}

/** A text node with each key of a known project as its link. */
function withKeys(text: string, keys: string, at: number): ReactNode {
  const parts = splitKeys(text, keys);
  if (parts.length === 1 && typeof parts[0] === "string") return text;
  return (
    <span key={at}>
      {parts.map((p, n) => (typeof p === "string" ? p : <IssueKeyLink key={n} issueKey={p.key} />))}
    </span>
  );
}

function block(b: Block, i: number, keys: string): ReactNode {
  const each = (c: Block, n: number) => block(c, n, keys);
  switch (b.t) {
    case "h": {
      const H = `h${b.level}` as "h1";
      return <H key={i}>{inlines(b.c, keys)}</H>;
    }
    case "p":
      return <p key={i}>{inlines(b.c, keys)}</p>;
    // GitHub's suggested change: said as one, since it is applied, not read.
    case "pre":
      return b.lang === "suggestion" ? (
        <div key={i} className="suggestion">
          <span>Suggested change</span>
          <pre>
            <code>{b.v}</code>
          </pre>
        </div>
      ) : (
        <pre key={i}>
          <code>{b.v}</code>
        </pre>
      );
    case "quote":
      return <blockquote key={i}>{b.c.map(each)}</blockquote>;
    case "hr":
      return <hr key={i} />;
    case "table":
      return (
        <div key={i} className="md-table">
          <table>
            <thead>
              <tr>
                {b.head.map((c, n) => (
                  <th key={n} style={{ textAlign: b.align[n] ?? undefined }}>
                    {inlines(c, keys)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {b.rows.map((row, r) => (
                <tr key={r}>
                  {row.map((c, n) => (
                    <td key={n} style={{ textAlign: b.align[n] ?? undefined }}>
                      {inlines(c, keys)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "list": {
      const items = b.items.map((item, n) => {
        // A one-paragraph item sits on its bullet's line, as GitHub's do.
        const [head, ...rest] = item.c;
        const lead = head?.t === "p" ? inlines(head.c, keys) : head ? each(head, 0) : null;
        return (
          <li key={n} className={item.task === null ? undefined : "md-task"}>
            {item.task !== null && <span className="md-box">{item.task ? "☑" : "☐"}</span>}
            {lead}
            {rest.map(each)}
          </li>
        );
      });
      return b.ordered ? (
        <ol key={i} start={b.start}>
          {items}
        </ol>
      ) : (
        <ul key={i}>{items}</ul>
      );
    }
  }
}
