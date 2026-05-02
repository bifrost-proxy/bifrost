import type { DebugDomNode } from "../../../api/devtools";
import { includesSearch } from "./sharedUtils";

export function domAttributes(attributes: DebugDomNode["attributes"]): Array<[string, string]> {
  if (!attributes) return [];
  if (Array.isArray(attributes)) {
    const pairs: Array<[string, string]> = [];
    for (let index = 0; index < attributes.length; index += 2) {
      pairs.push([String(attributes[index]), String(attributes[index + 1] ?? "")] as [string, string]);
    }
    return pairs;
  }
  return Object.entries(attributes)
    .map(([key, value]) => [key, String(value)] as [string, string]);
}

export function domNodeDisplayName(node: DebugDomNode): string {
  return String(node.nodeName ?? node["name"] ?? "node").toLowerCase();
}

export function domNodeKey(node: DebugDomNode, path: string): string {
  return node.nodeId != null ? `node:${node.nodeId}` : `path:${path}`;
}

export function domNodeSearchText(node: DebugDomNode): string {
  const attrs = domAttributes(node.attributes).flat().join(" ");
  return [domNodeDisplayName(node), node.nodeValue, attrs].filter(Boolean).join(" ");
}

export function findFirstDomSearchMatch(root: DebugDomNode, query: string): { node: DebugDomNode; expandedKeys: string[] } | null {
  const walk = (node: DebugDomNode, path: string, ancestors: string[]): { node: DebugDomNode; expandedKeys: string[] } | null => {
    if (includesSearch(domNodeSearchText(node), query)) {
      return { node, expandedKeys: ancestors };
    }
    const children = visibleDomChildren(node);
    const nextAncestors = children.length ? [...ancestors, domNodeKey(node, path)] : ancestors;
    for (let index = 0; index < children.length; index += 1) {
      const match = walk(children[index], `${path}.${index}`, nextAncestors);
      if (match) return match;
    }
    return null;
  };
  const roots = domTreeRoots(root);
  for (let index = 0; index < roots.length; index += 1) {
    const match = walk(roots[index], `0.${index}`, []);
    if (match) return match;
  }
  return null;
}

export function findDomNodePathById(root: DebugDomNode, nodeId: number): { node: DebugDomNode; expandedKeys: string[] } | null {
  const walk = (node: DebugDomNode, path: string, ancestors: string[]): { node: DebugDomNode; expandedKeys: string[] } | null => {
    if (node.nodeId === nodeId) {
      return { node, expandedKeys: ancestors };
    }
    const children = visibleDomChildren(node);
    const nextAncestors = children.length ? [...ancestors, domNodeKey(node, path)] : ancestors;
    for (let index = 0; index < children.length; index += 1) {
      const match = walk(children[index], `${path}.${index}`, nextAncestors);
      if (match) return match;
    }
    return null;
  };
  const roots = domTreeRoots(root);
  for (let index = 0; index < roots.length; index += 1) {
    const match = walk(roots[index], `0.${index}`, []);
    if (match) return match;
  }
  return null;
}

export function domTreeRoots(node: DebugDomNode): DebugDomNode[] {
  if (node.nodeType === 9) {
    return visibleDomChildren(node);
  }
  return shouldRenderDomNode(node) ? [node] : [];
}

export function visibleDomChildren(node: DebugDomNode): DebugDomNode[] {
  return (node.children ?? []).filter(shouldRenderDomNode);
}

function shouldRenderDomNode(node: DebugDomNode): boolean {
  if (node.nodeType !== 1 && node.nodeType !== 3) {
    return false;
  }
  if (node.nodeType === 3) {
    return formatTextNode(node.nodeValue) !== "";
  }
  return true;
}

export function isVoidDomNode(node: DebugDomNode): boolean {
  return ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"].includes(domNodeDisplayName(node));
}

export function formatTextNode(value: unknown): string {
  const text = typeof value === "string" ? value : "";
  return text.replace(/\s+/g, " ").trim();
}

export function collectDefaultExpandedDomKeys(root: DebugDomNode): Set<string> {
  const keys = new Set<string>();
  const walk = (node: DebugDomNode, path: string, depth: number) => {
    if (depth > 2) return;
    const children = visibleDomChildren(node);
    if (children.length > 0 && !isVoidDomNode(node)) {
      keys.add(domNodeKey(node, path));
    }
    children.slice(0, 12).forEach((child, index) => walk(child, `${path}.${index}`, depth + 1));
  };
  domTreeRoots(root).forEach((node, index) => walk(node, `0.${index}`, 0));
  return keys;
}
