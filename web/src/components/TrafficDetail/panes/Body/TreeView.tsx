import { useMemo, useState, useCallback, useEffect, useLayoutEffect, useRef } from 'react';
import { Tree, Typography, theme, ConfigProvider } from 'antd';
import type { DataNode } from 'antd/es/tree';
import type { SessionTargetSearchState } from '../../../../types';
import { MAX_JSON_FORMAT_HIGHLIGHT_LENGTH } from '../../helper/contentType';
import { useI18n } from '../../../../i18n';

const { Text } = Typography;

interface TreeViewProps {
  data?: string | null;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
}

const isObject = (val: unknown): val is Record<string, unknown> =>
  val !== null && typeof val === 'object' && !Array.isArray(val);

interface TreeNodeData {
  key: string;
  parentKey?: string;
  label: string;
  value: string;
  isExpandable: boolean;
  children?: TreeNodeData[];
}

const buildTreeNodeData = (
  data: unknown,
  path: string = 'root'
): TreeNodeData[] => {
  if (Array.isArray(data)) {
    return data.map((item, index) => {
      const key = `${path}-${index}`;
      const isExpandable = typeof item === 'object' && item !== null;
      return {
        key,
        parentKey: path,
        label: `[${index}]`,
        value: isExpandable
          ? Array.isArray(item)
            ? `Array(${item.length})`
            : `Object(${Object.keys(item).length})`
          : String(item),
        isExpandable,
        children: isExpandable ? buildTreeNodeData(item, key) : undefined,
      };
    });
  }

  if (isObject(data)) {
    return Object.entries(data).map(([k, v]) => {
      const key = `${path}-${k}`;
      const isExpandable = typeof v === 'object' && v !== null;
      return {
        key,
        parentKey: path,
        label: k,
        value: isExpandable
          ? Array.isArray(v)
            ? `Array(${v.length})`
            : `Object(${Object.keys(v as object).length})`
          : String(v),
        isExpandable,
        children: isExpandable ? buildTreeNodeData(v, key) : undefined,
      };
    });
  }

  return [];
};

interface MatchItem {
  index: number;
  nodeKey: string;
  expandKeys: React.Key[];
}

const findMatchPositions = (text: string, query: string) => {
  if (!query) return [];
  const t = text.toLowerCase();
  const q = query.toLowerCase();
  if (!q) return [];
  const positions: number[] = [];
  let startIndex = 0;
  while (startIndex <= t.length - q.length) {
    const idx = t.indexOf(q, startIndex);
    if (idx === -1) break;
    positions.push(idx);
    startIndex = idx + q.length;
  }
  return positions;
};

const renderHighlightedText = (
  text: string,
  query: string | undefined,
  matchIndices: number[],
  currentIndex: number
) => {
  if (!query || !query.length || matchIndices.length === 0) {
    return text;
  }

  const positions = findMatchPositions(text, query);
  if (positions.length === 0) {
    return text;
  }

  const qLen = query.length;
  const parts: React.ReactNode[] = [];
  let last = 0;
  positions.forEach((pos, i) => {
    if (pos > last) {
      parts.push(text.slice(last, pos));
    }
    const matchIndex = matchIndices[i];
    parts.push(
      <mark
        key={`${pos}-${matchIndex}`}
        data-bifrost-match-index={matchIndex}
        className={matchIndex === currentIndex ? 'mark-current' : undefined}
      >
        {text.slice(pos, pos + qLen)}
      </mark>
    );
    last = pos + qLen;
  });
  if (last < text.length) {
    parts.push(text.slice(last));
  }
  return parts;
};

const buildTreeDataAndMatches = (
  nodes: TreeNodeData[],
  query: string | undefined,
  currentIndex: number
): { treeData: DataNode[]; matches: MatchItem[] } => {
  const matches: MatchItem[] = [];
  let counter = 0;

  const walk = (ns: TreeNodeData[], path: React.Key[]): DataNode[] => {
    return ns.map((node) => {
      const currentPath = [...path, node.key];
      const labelPositions = query ? findMatchPositions(node.label, query) : [];
      const valuePositions = query ? findMatchPositions(node.value, query) : [];

      const labelMatchIndices = labelPositions.map(() => {
        counter += 1;
        matches.push({
          index: counter,
          nodeKey: node.key,
          expandKeys: currentPath.slice(0, -1),
        });
        return counter;
      });

      const valueMatchIndices = valuePositions.map(() => {
        counter += 1;
        matches.push({
          index: counter,
          nodeKey: node.key,
          expandKeys: currentPath.slice(0, -1),
        });
        return counter;
      });

      return {
        key: node.key,
        title: (
          <span data-bifrost-tree-key={node.key}>
            <Text strong>
              {renderHighlightedText(
                node.label,
                query,
                labelMatchIndices,
                currentIndex
              )}
              :{' '}
            </Text>
            {node.isExpandable ? (
              <Text type="secondary">
                {renderHighlightedText(
                  node.value,
                  query,
                  valueMatchIndices,
                  currentIndex
                )}
              </Text>
            ) : (
              <Text>
                {renderHighlightedText(
                  node.value,
                  query,
                  valueMatchIndices,
                  currentIndex
                )}
              </Text>
            )}
          </span>
        ),
        children: node.children ? walk(node.children, currentPath) : undefined,
      };
    });
  };

  return { treeData: walk(nodes, []), matches };
};

const parseJsonSafe = (data: string): { parsed: unknown; error: boolean } => {
  try {
    return { parsed: JSON.parse(data), error: false };
  } catch {
    return { parsed: null, error: true };
  }
};

export const TreeView = ({ data, searchValue, onSearch }: TreeViewProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [manualExpandedKeys, setManualExpandedKeys] = useState<React.Key[]>([
    'root',
  ]);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  const parsedData = useMemo(() => {
    if (!data) return { parsed: null, error: false };
    return parseJsonSafe(data);
  }, [data]);

  const treeNodeData = useMemo<TreeNodeData[]>(() => {
    if (!parsedData.parsed || parsedData.error) return [];
    const parsed = parsedData.parsed;
    const rootLabel = Array.isArray(parsed)
      ? `Array(${parsed.length})`
      : `Object(${Object.keys(parsed as object).length})`;

    return [
      {
        key: 'root',
        label: t('trafficDetail.tree.root'),
        value: rootLabel,
        isExpandable: true,
        children: buildTreeNodeData(parsed, 'root'),
      },
    ];
  }, [parsedData, t]);

  const currentIndex = useMemo(() => {
    const next = searchValue.next ?? 1;
    const v = searchValue.value;
    if (!v) return 0;
    if (!Number.isFinite(next) || next < 1) return 1;
    return Math.floor(next);
  }, [searchValue.next, searchValue.value]);

  const { treeData, matches } = useMemo(() => {
    return buildTreeDataAndMatches(treeNodeData, searchValue.value, currentIndex);
  }, [treeNodeData, searchValue.value, currentIndex]);

  const totalMatches = matches.length;
  const safeCurrentIndex = useMemo(() => {
    if (!searchValue.value || totalMatches === 0) return 0;
    const next = searchValue.next ?? 1;
    const t = Math.min(totalMatches, Math.max(1, Math.floor(next)));
    return t;
  }, [searchValue.value, searchValue.next, totalMatches]);

  const searchExpandKeys = useMemo(() => {
    if (!searchValue.value) return [];
    if (safeCurrentIndex === 0 || totalMatches === 0) return [];
    return matches[safeCurrentIndex - 1]?.expandKeys ?? [];
  }, [matches, safeCurrentIndex, searchValue.value, totalMatches]);

  const expandedKeys = useMemo(() => {
    const set = new Set<React.Key>();
    manualExpandedKeys.forEach((k) => set.add(k));
    searchExpandKeys.forEach((k) => set.add(k));
    return Array.from(set);
  }, [manualExpandedKeys, searchExpandKeys]);

  const onExpand = useCallback((keys: React.Key[]) => {
    setManualExpandedKeys(keys);
  }, []);

  useEffect(() => {
    if (!searchValue.value) {
      if ((searchValue.total ?? 0) !== 0) {
        onSearch({ total: 0 });
      }
      return;
    }

    if ((searchValue.total ?? 0) !== totalMatches) {
      onSearch({ total: totalMatches });
    }

    if (totalMatches > 0) {
      const next = searchValue.next ?? 1;
      const t = Math.min(totalMatches, Math.max(1, Math.floor(next)));
      if (t !== next) {
        onSearch({ next: t });
      }
    }
  }, [
    onSearch,
    searchValue.next,
    searchValue.total,
    searchValue.value,
    totalMatches,
  ]);

  useLayoutEffect(() => {
    if (!searchValue.value) return;
    if (safeCurrentIndex === 0 || totalMatches === 0) return;
    const el = wrapRef.current;
    if (!el) return;
    const id = safeCurrentIndex;
    const handle = requestAnimationFrame(() => {
      const mark = el.querySelector(
        `mark[data-bifrost-match-index="${id}"]`
      ) as HTMLElement | null;
      mark?.scrollIntoView?.({ block: 'center', behavior: 'smooth' });
    });
    return () => cancelAnimationFrame(handle);
  }, [safeCurrentIndex, searchValue.value, totalMatches, expandedKeys, treeData]);

  if (!data) {
    return null;
  }

  if (data.length > MAX_JSON_FORMAT_HIGHLIGHT_LENGTH) {
    return (
      <div style={{ padding: 12, color: token.colorTextSecondary }}>
        {t('trafficDetail.tree.tooLarge')}
      </div>
    );
  }

  if (parsedData.error || treeData.length === 0) {
    return (
      <div style={{ padding: 12, color: token.colorTextSecondary }}>
        {t('trafficDetail.tree.parseFailed')}
      </div>
    );
  }

  return (
    <div
      ref={wrapRef}
      style={{
        padding: 4,
        backgroundColor: token.colorBgLayout,
        borderRadius: 4,
      }}
    >
      <ConfigProvider
        theme={{
          components: {
            Tree: {
              titleHeight: 20,
              nodeHoverBg: 'transparent',
              nodeSelectedBg: 'transparent',
            },
          },
        }}
      >
        <Tree
          treeData={treeData}
          expandedKeys={expandedKeys}
          onExpand={onExpand}
          showLine
          selectable={false}
          style={{ background: 'transparent' }}
        />
      </ConfigProvider>
    </div>
  );
};
