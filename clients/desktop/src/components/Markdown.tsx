import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

// Agent text is untrusted: raw HTML is never rendered (react-markdown's
// default), remote images are not fetched, and links open outside the app.
const components: Components = {
  a: ({ href, children }) => (
    <a className="text-primary underline underline-offset-2" href={href} target="_blank" rel="noreferrer noopener">
      {children}
    </a>
  ),
  img: ({ alt }) => <span className="text-muted-foreground">[image{alt ? `: ${alt}` : ""}]</span>,
  p: ({ children }) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-5">{children}</ol>,
  li: ({ children }) => <li className="my-0.5">{children}</li>,
  h1: ({ children }) => <h1 className="mt-3 mb-2 text-lg font-semibold">{children}</h1>,
  h2: ({ children }) => <h2 className="mt-3 mb-2 text-base font-semibold">{children}</h2>,
  h3: ({ children }) => <h3 className="mt-3 mb-1.5 font-semibold">{children}</h3>,
  blockquote: ({ children }) => (
    <blockquote className="text-muted-foreground my-2 border-l-2 pl-3">{children}</blockquote>
  ),
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="text-xs">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border px-2 py-1 text-left font-medium">{children}</th>,
  td: ({ children }) => <td className="border px-2 py-1">{children}</td>,
  pre: ({ children }) => (
    <pre className="bg-muted my-2 overflow-x-auto rounded-md p-3 font-mono text-xs leading-5">{children}</pre>
  ),
  code: ({ className, children }) => {
    const lang = /language-(\S+)/.exec(className ?? "")?.[1];
    // Fenced blocks arrive inside <pre>; inline code does not carry a language
    // or a newline.
    const block = lang !== undefined || String(children).includes("\n");
    return block ? (
      <code data-language={lang}>{children}</code>
    ) : (
      <code className="bg-muted rounded px-1 py-0.5 font-mono text-[0.85em]">{children}</code>
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
