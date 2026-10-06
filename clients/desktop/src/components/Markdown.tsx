import { createContext, useContext } from "react";
import { ArrowUpRight } from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { workspaceFile, type WorkspaceInfo } from "@zwiim/sdk";
import { isOpenableLink, openLink } from "@/lib/bridge";
import { cn } from "@/lib/utils";

/** A bare URL reads without its scheme; the full address stays in the tooltip. */
export function linkText(href: string | undefined, children: React.ReactNode): React.ReactNode {
  const text = typeof children === "string" ? children : Array.isArray(children) && children.length === 1 ? children[0] : null;
  if (!href || text !== href || !/^https?:\/\//i.test(href)) return children;
  return href.replace(/^https?:\/\//i, "").replace(/\/$/, "");
}

/** Where a conversation's file links open: its workspace's files, beside it. */
export interface FileLinks {
  workspace: WorkspaceInfo;
  open(path: string): void;
  /** Opens a file with its uncommitted changes marked. */
  openDiff(path: string): void;
}

export const FileLinksContext = createContext<FileLinks | null>(null);

export { workspaceFile };

// Agent text is untrusted: raw HTML is never rendered (react-markdown's
// default), remote images are not fetched, web and mail links open in the
// default browser, and file links only inside the session's workspace.
const components: Components = {
  a: ({ href, children }) => {
    const files = useContext(FileLinksContext);
    const file = files && workspaceFile(href, files.workspace);
    const web = isOpenableLink(href) && /^https?:/i.test(href);
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
  img: ({ alt }) => <span className="text-muted-foreground">[image{alt ? `: ${alt}` : ""}]</span>,
  p: ({ children }) => <p className="my-2.5 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-5">{children}</ol>,
  li: ({ children }) => <li className="my-0.5">{children}</li>,
  h1: ({ children }) => <h1 className="mt-5 mb-2 text-lg font-semibold tracking-tight first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-5 mb-2 text-base font-semibold tracking-tight first:mt-0">{children}</h2>,
  h3: ({ children }) => <h3 className="mt-4 mb-1.5 font-semibold first:mt-0">{children}</h3>,
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
  pre: ({ children }) => (
    <pre className="bg-code my-3 overflow-x-auto rounded-lg px-4 py-3 font-mono text-xs leading-5">{children}</pre>
  ),
  code: ({ className, children }) => {
    const lang = /language-(\S+)/.exec(className ?? "")?.[1];
    // Fenced blocks arrive inside <pre>; inline code does not carry a language
    // or a newline.
    const block = lang !== undefined || String(children).includes("\n");
    return block ? (
      <code data-language={lang}>{children}</code>
    ) : (
      <code className="bg-muted rounded px-1.5 py-0.5 font-mono text-[0.85em]">{children}</code>
    );
  },
};

export function Markdown({ text, className }: { text: string; className?: string }) {
  return (
    <div className={cn("break-words", className)} data-testid="markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
