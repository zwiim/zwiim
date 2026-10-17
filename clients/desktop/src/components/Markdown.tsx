import { useContext, useEffect, useRef, useState } from "react";
import { ArrowUpRight, Check, Copy } from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { workspaceFile } from "@zwiim/sdk";
import { opensInPreview, previewKind } from "@zwiim/preview";
import { fileIcon } from "@/panels/FilesPanel";
import { AgentImageChip } from "@/attachments/AgentImageChip";
import { isOpenableLink, openLink } from "@/lib/bridge";
import { FileLinksContext } from "@/lib/file-links";
import { cn } from "@/lib/utils";

/** A bare URL reads without its scheme; the full address stays in the tooltip. */
export function linkText(href: string | undefined, children: React.ReactNode): React.ReactNode {
  const text =
    typeof children === "string" ? children : Array.isArray(children) && children.length === 1 ? children[0] : null;
  if (!href || text !== href || !/^https?:\/\//i.test(href)) return children;
  return href.replace(/^https?:\/\//i, "").replace(/\/$/, "");
}

export { FileLinksContext, type FileLinks } from "@/lib/file-links";

export { workspaceFile };

// Agent text is untrusted: raw HTML is never rendered (react-markdown's
// default), remote images are not fetched, web and mail links open in the
// default browser, and file links only inside the session's workspace.
const components: Components = {
  a: ({ href, children }) => {
    const files = useContext(FileLinksContext);
    const file = files && workspaceFile(href, files.workspace);
    const web = isOpenableLink(href) && /^https?:/i.test(href);
    if (file && opensInPreview(file)) {
      const name = file.split("/").pop() ?? file;
      const { icon: Icon, color } = fileIcon(name);
      return (
        <button
          type="button"
          className="bg-muted hover:bg-accent hover:text-foreground inline-flex max-w-56 items-center gap-1.5 rounded-md px-1.5 py-0.5 align-middle font-mono text-xs transition-colors"
          title={file}
          onClick={() => files.open(file)}
          data-testid="file-link-chip"
        >
          <Icon className={cn("size-3.5 shrink-0", color)} />
          <span className="min-w-0 truncate">{name}</span>
        </button>
      );
    }
    return (
      <a
        className={
          isOpenableLink(href) || file
            ? "text-brand underline-offset-2 hover:underline"
            : "text-muted-foreground cursor-default underline underline-offset-2"
        }
        href={href}
        title={file ?? href}
        target="_blank"
        rel="noreferrer noopener"
        onClick={(e) => {
          // The webview would navigate the app itself or drop the click.
          e.preventDefault();
          if (file) files.open(file);
          else if (isOpenableLink(href)) void openLink(href).catch(() => {});
        }}
      >
        {linkText(href, children)}
        {web && <ArrowUpRight className="ml-0.5 inline size-3 align-[-1px]" aria-hidden />}
      </a>
    );
  },
  img: ({ src, alt }) => {
    const files = useContext(FileLinksContext);
    const file = files && typeof src === "string" ? workspaceFile(src, files.workspace) : null;
    // A workspace image shows as a chip that opens the preview; nothing remote is fetched.
    if (file && previewKind(file) === "image") return <AgentImageChip path={file} />;
    return <span className="text-muted-foreground">[image{alt ? `: ${alt}` : ""}]</span>;
  },
  strong: ({ children }) => <strong className="text-prose-strong font-semibold">{children}</strong>,
  p: ({ children }) => <p className="my-2.5 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-5">{children}</ol>,
  li: ({ children }) => <li className="my-0.5">{children}</li>,
  h1: ({ children }) => (
    <h1 className="text-prose-strong mt-5 mb-2 text-lg font-semibold tracking-tight first:mt-0">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="text-prose-strong mt-5 mb-2 text-base font-semibold tracking-tight first:mt-0">{children}</h2>
  ),
  h3: ({ children }) => <h3 className="text-prose-strong mt-4 mb-1.5 font-semibold first:mt-0">{children}</h3>,
  blockquote: ({ children }) => (
    <blockquote className="text-muted-foreground my-2 border-l-2 pl-3">{children}</blockquote>
  ),
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="border-collapse text-xs">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border-b px-2.5 py-1.5 text-left font-medium">{children}</th>,
  td: ({ children }) => <td className="border-b px-2.5 py-1.5">{children}</td>,
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  code: ({ className, children }) => {
    const lang = /language-(\S+)/.exec(className ?? "")?.[1];
    // Fenced blocks arrive inside <pre>; inline code does not carry a language
    // or a newline.
    const block = lang !== undefined || String(children).includes("\n");
    return block ? (
      <code data-language={lang}>{children}</code>
    ) : (
      <code className="bg-muted rounded-sm px-1.5 py-0.5 font-mono text-[0.85em]">{children}</code>
    );
  },
};

/** A fenced block, with a Copy button that shows on hover or keyboard focus. */
function CodeBlock({ children }: { children: React.ReactNode }) {
  const pre = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <div className="group relative my-3">
      <pre ref={pre} className="bg-code overflow-x-auto rounded-lg px-4 py-3 font-mono text-xs leading-5">
        {children}
      </pre>
      <button
        type="button"
        className="bg-popover text-muted-foreground hover:text-foreground ease-std absolute top-2 right-2 inline-flex h-6 items-center gap-1 rounded-md border px-2 text-xs opacity-0 transition-[opacity,color] duration-120 group-focus-within:opacity-100 group-hover:opacity-100 focus-visible:opacity-100"
        aria-label={copied ? "Copied" : "Copy code"}
        onClick={() => {
          const text = pre.current?.textContent ?? "";
          void navigator.clipboard?.writeText(text).then(() => setCopied(true));
        }}
      >
        {copied ? <Check className="text-done size-3.5" /> : <Copy className="size-3.5" />}
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

export function Markdown({ text, className }: { text: string; className?: string }) {
  return (
    <div className={cn("text-prose break-words", className)} data-testid="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
