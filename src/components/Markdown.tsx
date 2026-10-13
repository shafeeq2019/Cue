import { useMemo, useState } from "react";
import { parseBlocks, type Block, type Inline } from "../markdown";

function InlineNodes({ nodes }: { nodes: Inline[] }) {
  return (
    <>
      {nodes.map((n, i) => {
        switch (n.t) {
          case "text":
            return <span key={i}>{n.v}</span>;
          case "code":
            return <code key={i}>{n.v}</code>;
          case "strong":
            return (
              <strong key={i}>
                <InlineNodes nodes={n.c} />
              </strong>
            );
          case "em":
            return (
              <em key={i}>
                <InlineNodes nodes={n.c} />
              </em>
            );
          case "link":
            // Links are shown, not opened: answers can contain arbitrary URLs.
            return (
              <span key={i} className="md-link" title={n.href}>
                {n.text}
                {n.text !== n.href && <span className="md-href"> ({n.href})</span>}
              </span>
            );
        }
      })}
    </>
  );
}

function CodeBlock({ lang, code, onCopy }: { lang: string; code: string; onCopy: (t: string) => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="md-code">
      <div className="md-code-head">
        <span>{lang || "code"}</span>
        <button
          className="ghost tiny"
          onClick={() => {
            onCopy(code);
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}

function BlockNode({ b, onCopy }: { b: Block; onCopy: (t: string) => void }) {
  switch (b.t) {
    case "p":
      return (
        <p>
          <InlineNodes nodes={b.c} />
        </p>
      );
    case "h": {
      const Tag = (`h${Math.min(b.level + 2, 6)}`) as "h3";
      return (
        <Tag>
          <InlineNodes nodes={b.c} />
        </Tag>
      );
    }
    case "code":
      return <CodeBlock lang={b.lang} code={b.v} onCopy={onCopy} />;
    case "ul":
      return (
        <ul>
          {b.items.map((it, i) => (
            <li key={i}>
              <InlineNodes nodes={it} />
            </li>
          ))}
        </ul>
      );
    case "ol":
      return (
        <ol start={b.start}>
          {b.items.map((it, i) => (
            <li key={i}>
              <InlineNodes nodes={it} />
            </li>
          ))}
        </ol>
      );
    case "quote":
      return (
        <blockquote>
          <InlineNodes nodes={b.c} />
        </blockquote>
      );
    case "hr":
      return <hr />;
  }
}

export function Markdown({ text, onCopy }: { text: string; onCopy: (t: string) => void }) {
  const blocks = useMemo(() => parseBlocks(text), [text]);
  return (
    <div className="md">
      {blocks.map((b, i) => (
        <BlockNode key={i} b={b} onCopy={onCopy} />
      ))}
    </div>
  );
}
