import { memo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { highlighted } from "./highlight";
import { post } from "../../bridge";
import { CopyButton } from "../../ui/CopyButton";

function CodeBlock({ code, language }: { code: string; language: string | undefined }) {
  const html = highlighted(code, language);
  return (
    <div className="code">
      <div className="code-head">
        <span>{language || "code"}</span>
        <CopyButton text={code} />
      </div>
      <pre>
        {html === null ? <code>{code}</code> : <code dangerouslySetInnerHTML={{ __html: html }} />}
      </pre>
    </div>
  );
}

/** An answer as Markdown. Raw HTML in it is not interpreted, images are not loaded, and a
 * link only reports itself: the app decides whether and how it opens. */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a({ href, children }) {
            return (
              <a
                href="#"
                title={href}
                onClick={(event) => {
                  event.preventDefault();
                  if (href) post({ type: "link", url: href, open: event.metaKey || event.ctrlKey });
                }}
              >
                {children}
              </a>
            );
          },
          img({ alt }) {
            return <span className="muted">{alt || "image"}</span>;
          },
          pre({ children }) {
            return <>{children}</>;
          },
          code({ className, children }) {
            const text = String(children);
            const language = /language-([\w+#-]+)/.exec(className ?? "")?.[1];
            if (language !== undefined || text.includes("\n")) {
              return <CodeBlock code={text.replace(/\n$/, "")} language={language} />;
            }
            return <code className="inline">{children}</code>;
          },
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});
