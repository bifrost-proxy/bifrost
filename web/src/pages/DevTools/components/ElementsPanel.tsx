import { useState, type CSSProperties } from "react";
import { CaretDownOutlined, CaretRightOutlined } from "@ant-design/icons";
import { Button, Empty, Modal, Typography, message } from "antd";
import type { DebugDomNode } from "../../../api/devtools";
import { HighlightedText } from "./shared";
import {
  domAttributes,
  domNodeDisplayName,
  domNodeKey,
  domNodeSearchText,
  domTreeRoots,
  formatTextNode,
  isVoidDomNode,
  visibleDomChildren,
} from "./domUtils";
import { includesSearch } from "./sharedUtils";

const { Text } = Typography;
const PREVIEW_LIMIT = 120;

type NodeDetail = {
  title: string;
  value: string;
};

export function DomTree({
  node,
  html,
  selectedNodeId,
  expandedKeys,
  searchQuery,
  onHighlight,
  onToggle,
}: {
  node: DebugDomNode | null;
  html: string;
  selectedNodeId: number | null;
  expandedKeys: Set<string>;
  searchQuery: string;
  onHighlight: (nodeId: number) => void;
  onToggle: (key: string) => void;
}) {
  const [detail, setDetail] = useState<NodeDetail | null>(null);
  const showDetail = (title: string, value: string) => setDetail({ title, value });
  if (node) {
    const roots = domTreeRoots(node);
    return (
      <>
        <div data-testid="devtools-elements-tree" style={treePaneStyle}>
          {roots.map((root, index) =>
            renderDomNode(root, `0.${index}`, selectedNodeId, expandedKeys, searchQuery, onHighlight, onToggle, showDetail),
          )}
        </div>
        <Modal
          title={detail?.title}
          open={Boolean(detail)}
          width="min(920px, 92vw)"
          footer={(
            <Button
              data-testid="devtools-elements-detail-copy"
              onClick={() => {
                void navigator.clipboard.writeText(detail?.value ?? "").then(() => {
                  void message.success("Copied");
                });
              }}
            >
              Copy
            </Button>
          )}
          onCancel={() => setDetail(null)}
        >
          <pre data-testid="devtools-elements-detail-value" style={detailValueStyle}>{detail?.value}</pre>
        </Modal>
      </>
    );
  }
  if (html) {
    return <pre style={codeBlockStyle}>{html}</pre>;
  }
  return <Empty description="No DOM snapshot yet" />;
}

function renderDomNode(
  node: DebugDomNode,
  path: string,
  selectedNodeId: number | null,
  expandedKeys: Set<string>,
  searchQuery: string,
  onHighlight: (nodeId: number) => void,
  onToggle: (key: string) => void,
  onShowDetail: (title: string, value: string) => void,
): React.ReactNode {
  const children = visibleDomChildren(node);
  const key = domNodeKey(node, path);
  const nodeId = node.nodeId;
  const selected = nodeId != null && nodeId === selectedNodeId;
  const matched = searchQuery.trim() && includesSearch(domNodeSearchText(node), searchQuery);
  const hasChildren = children.length > 0 && !isVoidDomNode(node);
  const expanded = !hasChildren || expandedKeys.has(key);
  const isElement = node.nodeType === 1;
  const isText = node.nodeType === 3;
  return (
    <div key={key} style={domBranchStyle}>
      <div
        style={{
          ...domLineStyle,
          background: selected ? "var(--devtools-selected-bg)" : matched ? "var(--devtools-search-bg)" : "transparent",
        }}
      >
        <button
          type="button"
          data-testid="devtools-dom-disclosure"
          aria-label={expanded ? "Collapse node" : "Expand node"}
          disabled={!hasChildren}
          onClick={(event) => {
            event.stopPropagation();
            if (hasChildren) onToggle(key);
          }}
          style={{
            ...domDisclosureStyle,
            visibility: hasChildren ? "visible" : "hidden",
          }}
        >
          {expanded ? <CaretDownOutlined /> : <CaretRightOutlined />}
        </button>
        {nodeId != null ? (
          <span
            role="button"
            tabIndex={0}
            data-testid="devtools-dom-node"
            data-selected={selected ? "true" : undefined}
            onClick={() => {
              if (hasChildren) onToggle(key);
              onHighlight(nodeId);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                if (hasChildren) onToggle(key);
                onHighlight(nodeId);
              }
            }}
            style={{
              ...domNodeButtonStyle,
              borderColor: selected ? "var(--devtools-selected-border)" : "transparent",
            }}
          >
            <DomNodePreview node={node} expanded={expanded} searchQuery={searchQuery} onShowDetail={onShowDetail} />
          </span>
        ) : (
          <span style={domNodeStaticStyle}>
            {isElement ? <DomNodePreview node={node} expanded={expanded} searchQuery={searchQuery} onShowDetail={onShowDetail} /> : null}
            {isText ? <TextNodePreview value={node.nodeValue} searchQuery={searchQuery} onShowDetail={onShowDetail} /> : null}
            {!isElement && !isText ? <span style={domTextNodeStyle}><HighlightedText text={domNodeDisplayName(node)} query={searchQuery} /></span> : null}
          </span>
        )}
      </div>
      {hasChildren && expanded ? (
        <div style={domChildrenStyle}>
          {children.slice(0, 250).map((child, index) =>
            renderDomNode(child, `${path}.${index}`, selectedNodeId, expandedKeys, searchQuery, onHighlight, onToggle, onShowDetail),
          )}
          {children.length > 250 ? <Text type="secondary">... {children.length - 250} more nodes</Text> : null}
          {isElement ? (
            <div style={domClosingTagStyle}>
              <span>&lt;/</span>
              <span style={domTagStyle}>{domNodeDisplayName(node)}</span>
              <span>&gt;</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function DomNodePreview({
  node,
  expanded,
  searchQuery,
  onShowDetail,
}: {
  node: DebugDomNode;
  expanded: boolean;
  searchQuery: string;
  onShowDetail: (title: string, value: string) => void;
}) {
  const name = domNodeDisplayName(node);
  const attrs = domAttributes(node.attributes);
  const children = visibleDomChildren(node);
  const showEllipsis = children.length > 0 && !expanded && !isVoidDomNode(node);
  if (node.nodeType !== 1) {
    return <TextNodePreview value={node.nodeValue} searchQuery={searchQuery} onShowDetail={onShowDetail} />;
  }
  return (
    <span style={domPreviewStyle}>
      <span>&lt;</span>
      <span style={domTagStyle}><HighlightedText text={name} query={searchQuery} /></span>
      {attrs.slice(0, 8).map(([key, value]) => (
        <span key={`${key}-${value}`}>
          {" "}
          <span style={domAttrNameStyle}><HighlightedText text={key} query={searchQuery} /></span>
          {value !== "" ? (
            <>
              <span>=&quot;</span>
              <PreviewValue
                title={`${name}.${key}`}
                value={value}
                searchQuery={searchQuery}
                style={domAttrValueStyle}
                onShowDetail={onShowDetail}
              />
              <span>&quot;</span>
            </>
          ) : null}
        </span>
      ))}
      {attrs.length > 8 ? <span style={domMutedStyle}> ...</span> : null}
      <span>{isVoidDomNode(node) ? " /" : ""}&gt;</span>
      {showEllipsis ? <span style={domMutedStyle}>...</span> : null}
      {!expanded && !isVoidDomNode(node) && children.length === 0 ? (
        <>
          <span>&lt;/</span>
          <span style={domTagStyle}>{name}</span>
          <span>&gt;</span>
        </>
      ) : null}
    </span>
  );
}

function TextNodePreview({
  value,
  searchQuery,
  onShowDetail,
}: {
  value: unknown;
  searchQuery: string;
  onShowDetail: (title: string, value: string) => void;
}) {
  const text = formatTextNode(value);
  if (!text) return null;
  return (
    <span style={domTextNodeStyle}>
      <PreviewValue title="Text node" value={text} searchQuery={searchQuery} quote onShowDetail={onShowDetail} />
    </span>
  );
}

function PreviewValue({
  title,
  value,
  searchQuery,
  style,
  quote,
  onShowDetail,
}: {
  title: string;
  value: string;
  searchQuery: string;
  style?: CSSProperties;
  quote?: boolean;
  onShowDetail: (title: string, value: string) => void;
}) {
  const preview = value.length > PREVIEW_LIMIT ? `${value.slice(0, PREVIEW_LIMIT)}...` : value;
  const display = quote ? `"${preview}"` : preview;
  return (
    <span style={style}>
      <HighlightedText text={display} query={searchQuery} />
      {value.length > PREVIEW_LIMIT ? (
        <button
          type="button"
          data-testid="devtools-dom-value-detail"
          style={detailTriggerStyle}
          onClick={(event) => {
            event.stopPropagation();
            onShowDetail(title, value);
          }}
        >
          view
        </button>
      ) : null}
    </span>
  );
}

const treePaneStyle: CSSProperties = {
  minWidth: 0,
  maxWidth: "100%",
  height: "100%",
  minHeight: 0,
  overflow: "auto",
  padding: 8,
  background: "var(--devtools-surface)",
  border: "1px solid var(--devtools-border)",
  color: "var(--devtools-text)",
  borderRadius: 6,
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 12,
  lineHeight: 1.55,
};

const domLineStyle: CSSProperties = {
  minHeight: 21,
  display: "flex",
  alignItems: "flex-start",
  minWidth: 0,
  maxWidth: "100%",
  paddingRight: 8,
};

const domBranchStyle: CSSProperties = {
  minWidth: 0,
  maxWidth: "100%",
};

const domChildrenStyle: CSSProperties = {
  marginLeft: 14,
  minWidth: 0,
  maxWidth: "100%",
};

const domDisclosureStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  width: 16,
  minWidth: 16,
  height: 18,
  padding: 0,
  border: 0,
  background: "transparent",
  color: "var(--devtools-text-secondary)",
  cursor: "pointer",
  fontSize: 10,
};

const domNodeButtonStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  display: "inline-flex",
  alignItems: "flex-start",
  minWidth: 0,
  maxWidth: "100%",
  minHeight: 20,
  padding: "1px 4px",
  border: "1px solid transparent",
  borderRadius: 3,
  background: "transparent",
  color: "inherit",
  textAlign: "left",
  cursor: "pointer",
};

const domNodeStaticStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "flex-start",
  minWidth: 0,
  maxWidth: "100%",
  minHeight: 20,
  padding: "1px 4px",
};

const domPreviewStyle: CSSProperties = {
  minWidth: 0,
  maxWidth: "100%",
  whiteSpace: "normal",
  overflowWrap: "anywhere",
  wordBreak: "break-word",
};

const domTagStyle: CSSProperties = {
  color: "var(--devtools-code-tag)",
};

const domAttrNameStyle: CSSProperties = {
  color: "var(--devtools-code-attr)",
};

const domAttrValueStyle: CSSProperties = {
  color: "var(--devtools-code-number)",
};

const domTextNodeStyle: CSSProperties = {
  color: "var(--devtools-text)",
  whiteSpace: "normal",
  overflowWrap: "anywhere",
  wordBreak: "break-word",
};

const domMutedStyle: CSSProperties = {
  color: "var(--devtools-text-secondary)",
};

const domClosingTagStyle: CSSProperties = {
  minHeight: 21,
  paddingLeft: 20,
  color: "var(--devtools-text)",
  whiteSpace: "normal",
  overflowWrap: "anywhere",
};

const codeBlockStyle: CSSProperties = {
  margin: 0,
  padding: 12,
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  background: "var(--devtools-surface-alt)",
  color: "var(--devtools-text)",
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

const detailTriggerStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  marginLeft: 6,
  padding: "0 4px",
  border: "1px solid var(--devtools-border-strong)",
  borderRadius: 3,
  background: "var(--devtools-surface-alt)",
  color: "var(--devtools-text-secondary)",
  fontSize: 10,
  lineHeight: "16px",
  cursor: "pointer",
};

const detailValueStyle: CSSProperties = {
  maxHeight: "70vh",
  margin: 0,
  padding: 12,
  overflow: "auto",
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  background: "var(--devtools-surface-alt)",
  color: "var(--devtools-text)",
  whiteSpace: "pre-wrap",
  overflowWrap: "anywhere",
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
  fontSize: 12,
  lineHeight: 1.55,
};
