import type { CSSProperties } from "react";
import { theme } from "antd";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

interface MarkdownContentProps {
  content: string;
  testId?: string;
  token: ReturnType<typeof theme.useToken>["token"];
}

export default function MarkdownContent({ content, testId, token }: MarkdownContentProps) {
  const style = {
    "--asr-markdown-border": token.colorBorderSecondary,
    "--asr-markdown-fill": token.colorFillQuaternary,
    "--asr-markdown-fill-strong": token.colorFillTertiary,
    "--asr-markdown-text": token.colorText,
    "--asr-markdown-text-secondary": token.colorTextSecondary,
    "--asr-markdown-link": token.colorPrimary,
  } as CSSProperties;

  return (
    <div className="asr-markdown-content" data-testid={testId} style={style}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
    </div>
  );
}
