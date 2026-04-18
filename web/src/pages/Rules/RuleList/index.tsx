import { useMemo, useState, useCallback, useRef, useEffect, type ReactNode } from 'react';
import {
  Input,
  Button,
  Dropdown,
  Modal,
  message,
  Switch,
  Tooltip,
  Spin,
  Select,
  Tag,
} from 'antd';
import type { MenuProps } from 'antd';
import {
  PlusOutlined,
  ReloadOutlined,
  SearchOutlined,
  CheckOutlined,
  EditOutlined,
  DeleteOutlined,
  PoweroffOutlined,
  ExportOutlined,
  HolderOutlined,
  SwapOutlined,
  CaretDownOutlined,
  CaretRightOutlined,
  FolderOutlined,
  FolderOpenOutlined,
} from '@ant-design/icons';
import { useRulesStore } from '../../../stores/useRulesStore';
import { ImportBifrostButton } from '../../../components/ImportBifrostButton';
import { useExportBifrost } from '../../../hooks/useExportBifrost';
import { useSyncStore } from '../../../stores/useSyncStore';
import { searchGroups, type Group } from '../../../api/group';
import styles from './index.module.css';
import {
  buildRuleTree,
  collectFolderPaths,
  flattenVisibleRuleNames,
  getRuleParentPaths,
  getTopFolderPrefix,
  type RuleTreeFolderNode,
} from './ruleTree';

type RuleSortMode = 'manual' | 'updated_desc' | 'name_asc';

const ruleSortOptions = [
  { label: 'Manual', value: 'manual' },
  { label: 'Updated', value: 'updated_desc' },
  { label: 'Name', value: 'name_asc' },
];

function getRuleItemId(name: string) {
  return `rule-item-${encodeURIComponent(name)}`;
}

export default function RuleList() {
  const {
    rules,
    selectedRuleName,
    searchKeyword,
    loading,
    isGroupMode,
    groupWritable,
    activeGroupId,
    setActiveGroupId,
    fetchRules,
    selectRule,
    createRule,
    deleteRule,
    toggleRule,
    renameRule,
    reorderRules,
    setSearchKeyword,
    hasUnsavedChanges,
  } = useRulesStore();

  const canEdit = !isGroupMode || groupWritable;
  const isReadOnlyGroup = isGroupMode && !groupWritable;

  const [createModalVisible, setCreateModalVisible] = useState(false);
  const [newRuleName, setNewRuleName] = useState('');
  const [renameModalVisible, setRenameModalVisible] = useState(false);
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [newName, setNewName] = useState('');
  const [selectedRules, setSelectedRules] = useState<string[]>([]);
  const lastClickedIndexRef = useRef<number | null>(null);
  const [sortMode, setSortMode] = useState<RuleSortMode>('manual');
  const [draggedRuleName, setDraggedRuleName] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<{
    name: string;
    position: 'before' | 'after';
  } | null>(null);
  const [expandedFolders, setExpandedFolders] = useState<string[]>([]);
  const expandedFolderManualSet = useMemo(() => new Set(expandedFolders), [expandedFolders]);
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const autoScrollFrameRef = useRef<number | null>(null);
  const autoScrollVelocityRef = useRef(0);
  const { exportFile } = useExportBifrost();

  const [userGroups, setUserGroups] = useState<Group[]>([]);
  const [showLoadingOverlay, setShowLoadingOverlay] = useState(false);
  const loadingTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const syncStatus = useSyncStore((state) => state.syncStatus);
  const showGroupSwitcher = !!(syncStatus?.enabled && syncStatus?.has_session && syncStatus?.authorized);

  useEffect(() => {
    if (loading) {
      const timer = setTimeout(() => {
        setShowLoadingOverlay(true);
      }, 500);
      loadingTimerRef.current = timer;
      return () => {
        clearTimeout(timer);
        loadingTimerRef.current = null;
      };
    }
    loadingTimerRef.current = null;
    const frame = requestAnimationFrame(() => {
      setShowLoadingOverlay(false);
    });
    return () => cancelAnimationFrame(frame);
  }, [loading]);

  useEffect(() => {
    if (!showGroupSwitcher) return;
    let cancelled = false;
    const loadGroups = async () => {
      try {
        const result = await searchGroups();
        if (!cancelled) setUserGroups(result.list ?? []);
      } catch {
        // keep existing groups on error
      }
    };
    loadGroups();
    return () => { cancelled = true; };
  }, [showGroupSwitcher]);

  useEffect(() => {
    fetchRules();
  }, [activeGroupId, fetchRules]);


  const handleGroupChange = useCallback((value: string) => {
    setExpandedFolders([]);
    setSelectedRules([]);
    lastClickedIndexRef.current = null;

    const groupId = value === '__my__' ? null : value;
    setActiveGroupId(groupId);
  }, [setActiveGroupId]);

  const groupSwitcherOptions = useMemo(() => {
    const options: { label: ReactNode; value: string; searchText: string }[] = [
      { label: 'My Rules', value: '__my__', searchText: 'My Rules' },
    ];

    const sorted = [...(userGroups ?? [])].sort((a, b) => {
      const rank = (g: Group) => {
        if (g.level === 2) return 0;
        if (g.level === 1 || g.level === 0) return 1;
        return 2;
      };
      return rank(a) - rank(b) || a.name.localeCompare(b.name);
    });

    const levelTag = (g: Group) => {
      if (g.level === 2) return <Tag color="gold" style={{ marginLeft: 'auto', marginRight: 0, lineHeight: '18px', fontSize: 11, padding: '0 4px' }}>Owner</Tag>;
      if (g.level === 1) return <Tag color="blue" style={{ marginLeft: 'auto', marginRight: 0, lineHeight: '18px', fontSize: 11, padding: '0 4px' }}>Master</Tag>;
      if (g.level === 0) return <Tag color="cyan" style={{ marginLeft: 'auto', marginRight: 0, lineHeight: '18px', fontSize: 11, padding: '0 4px' }}>Member</Tag>;
      return <Tag style={{ marginLeft: 'auto', marginRight: 0, lineHeight: '18px', fontSize: 11, padding: '0 4px' }}>Public</Tag>;
    };

    for (const g of sorted) {
      options.push({
        label: (
          <span style={{ display: 'flex', alignItems: 'center', gap: 4, width: '100%' }}>
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1 }}>{g.name}</span>
            {levelTag(g)}
          </span>
        ),
        value: g.id,
        searchText: g.name,
      });
    }
    return options;
  }, [userGroups]);

  const stopAutoScroll = useCallback(() => {
    if (autoScrollFrameRef.current !== null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
      autoScrollFrameRef.current = null;
    }
    autoScrollVelocityRef.current = 0;
  }, []);

  const startAutoScroll = useCallback(
    (velocity: number) => {
      autoScrollVelocityRef.current = velocity;
      if (autoScrollFrameRef.current !== null) {
        return;
      }

      const tick = () => {
        const container = listContainerRef.current;
        if (!container || autoScrollVelocityRef.current === 0) {
          autoScrollFrameRef.current = null;
          return;
        }

        const maxScrollTop = container.scrollHeight - container.clientHeight;
        const nextScrollTop = Math.max(
          0,
          Math.min(maxScrollTop, container.scrollTop + autoScrollVelocityRef.current)
        );
        container.scrollTop = nextScrollTop;

        if (
          nextScrollTop === 0 ||
          nextScrollTop === maxScrollTop
        ) {
          autoScrollFrameRef.current = null;
          return;
        }

        autoScrollFrameRef.current = requestAnimationFrame(tick);
      };

      autoScrollFrameRef.current = requestAnimationFrame(tick);
    },
    []
  );

  const updateAutoScroll = useCallback(
    (clientY: number) => {
      const container = listContainerRef.current;
      if (!container) return;

      const rect = container.getBoundingClientRect();
      const edgeThreshold = 40;
      const maxVelocity = 12;

      const distanceToTop = clientY - rect.top;
      const distanceToBottom = rect.bottom - clientY;

      if (distanceToTop >= 0 && distanceToTop < edgeThreshold) {
        const ratio = (edgeThreshold - distanceToTop) / edgeThreshold;
        startAutoScroll(-Math.max(4, Math.round(maxVelocity * ratio)));
        return;
      }

      if (distanceToBottom >= 0 && distanceToBottom < edgeThreshold) {
        const ratio = (edgeThreshold - distanceToBottom) / edgeThreshold;
        startAutoScroll(Math.max(4, Math.round(maxVelocity * ratio)));
        return;
      }

      stopAutoScroll();
    },
    [startAutoScroll, stopAutoScroll]
  );

  const filteredRules = useMemo(() => {
    const sortedRules = [...rules].sort((left, right) => {
      if (sortMode === 'updated_desc') {
        return (
          Date.parse(right.updated_at) - Date.parse(left.updated_at) ||
          left.name.localeCompare(right.name)
        );
      }
      if (sortMode === 'name_asc') {
        return left.name.localeCompare(right.name);
      }
      return left.sort_order - right.sort_order || left.name.localeCompare(right.name);
    });
    if (!searchKeyword) return sortedRules;
    const keyword = searchKeyword.toLowerCase();
    return sortedRules.filter((rule) => rule.name.toLowerCase().includes(keyword));
  }, [rules, searchKeyword, sortMode]);

  const ruleTree = useMemo(() => buildRuleTree(filteredRules), [filteredRules]);
  const allFolderPaths = useMemo(() => collectFolderPaths(ruleTree), [ruleTree]);

  const forcedExpandedFolderSet = useMemo(() => {
    if (!selectedRuleName) return new Set<string>();
    return new Set(getRuleParentPaths(selectedRuleName));
  }, [selectedRuleName]);

  const expandedFolderSet = useMemo(() => {
    if (searchKeyword) {
      return new Set(allFolderPaths);
    }

    const next = new Set(expandedFolderManualSet);
    for (const path of forcedExpandedFolderSet) {
      next.add(path);
    }
    return next;
  }, [searchKeyword, allFolderPaths, expandedFolderManualSet, forcedExpandedFolderSet]);

  const visibleRuleNames = useMemo(
    () => flattenVisibleRuleNames(ruleTree, expandedFolderSet),
    [ruleTree, expandedFolderSet]
  );

  const handleCreate = async () => {
    if (!newRuleName.trim()) {
      message.error('Rule name is required');
      return;
    }
    const success = await createRule(newRuleName.trim(), '# New rule\n');
    if (success) {
      message.success('Rule created');
      setCreateModalVisible(false);
      setNewRuleName('');
    }
  };

  const handleDelete = async (name: string) => {
    Modal.confirm({
      title: 'Delete Rule',
      content: `Are you sure to delete "${name}"?`,
      okText: 'Delete',
      okType: 'danger',
      cancelText: 'Cancel',
      onOk: async () => {
        const success = await deleteRule(name);
        if (success) {
          message.success('Rule deleted');
        }
      },
    });
  };

  const handleBulkDelete = async (names: string[]) => {
    if (names.length === 0) return;
    if (names.length === 1) {
      handleDelete(names[0]);
      return;
    }
    Modal.confirm({
      title: 'Delete Rules',
      content: `Are you sure to delete ${names.length} rules?`,
      okText: 'Delete',
      okType: 'danger',
      cancelText: 'Cancel',
      onOk: async () => {
        let successCount = 0;
        for (const name of names) {
          const success = await deleteRule(name);
          if (success) successCount++;
        }
        if (successCount > 0) {
          message.success(`${successCount} rule${successCount > 1 ? 's' : ''} deleted`);
          setSelectedRules([]);
        }
      },
    });
  };

  const handleToggle = async (name: string, enabled: boolean) => {
    const success = await toggleRule(name, enabled);
    if (success) {
      message.success(`Rule ${enabled ? 'enabled' : 'disabled'}`);
    }
  };

  const handleRename = async () => {
    if (!renameTarget || !newName.trim()) return;
    if (newName.trim() === renameTarget) {
      setRenameModalVisible(false);
      return;
    }
    const success = await renameRule(renameTarget, newName.trim());
    if (success) {
      message.success('Rule renamed');
      setRenameModalVisible(false);
      setRenameTarget(null);
      setNewName('');
    }
  };

  const handleExport = useCallback(
    async (names: string[]) => {
      if (names.length === 0) return;
      const filename =
        names.length === 1
          ? `${names[0]}.bifrost`
          : `bifrost-rules-${names.length}.bifrost`;
      await exportFile('rules', { rule_names: names }, filename);
    },
    [exportFile]
  );

  const handleImportSuccess = useCallback(() => {
    fetchRules();
  }, [fetchRules]);

  const handleSelect = useCallback(
    (name: string, e: React.MouseEvent) => {
      const isCtrl = e.ctrlKey || e.metaKey;
      const isShift = e.shiftKey;
      const currentIndex = visibleRuleNames.findIndex((ruleName) => ruleName === name);

      if (currentIndex === -1) {
        return;
      }

      if (isShift && lastClickedIndexRef.current !== null) {
        const start = Math.min(lastClickedIndexRef.current, currentIndex);
        const end = Math.max(lastClickedIndexRef.current, currentIndex);
        const rangeNames = visibleRuleNames.slice(start, end + 1);
        setSelectedRules((prev) => {
          const combined = new Set([...prev, ...rangeNames]);
          return Array.from(combined);
        });
      } else if (isCtrl) {
        setSelectedRules((prev) =>
          prev.includes(name) ? prev.filter((n) => n !== name) : [...prev, name]
        );
        lastClickedIndexRef.current = currentIndex;
      } else {
        setSelectedRules([]);
        lastClickedIndexRef.current = currentIndex;
        selectRule(name);
      }
    },
    [selectRule, visibleRuleNames]
  );

  const getContextMenuItems = (name: string, enabled: boolean): MenuProps['items'] => {
    const isSelected = selectedRules.includes(name);
    const bulkNames = isSelected && selectedRules.length > 0 ? selectedRules : [name];

    if (isReadOnlyGroup) {
      return [
        {
          key: 'toggle',
          icon: enabled ? <PoweroffOutlined /> : <CheckOutlined />,
          label: enabled ? 'Disable' : 'Enable',
          onClick: () => handleToggle(name, !enabled),
        },
        {
          type: 'divider',
        },
        {
          key: 'export',
          icon: <ExportOutlined />,
          label: `Export${bulkNames.length > 1 ? ` (${bulkNames.length})` : ''}`,
          onClick: () => handleExport(bulkNames),
        },
      ];
    }

    if (isGroupMode && groupWritable) {
      return [
        {
          key: 'toggle',
          icon: enabled ? <PoweroffOutlined /> : <CheckOutlined />,
          label: enabled ? 'Disable' : 'Enable',
          onClick: () => handleToggle(name, !enabled),
        },
        {
          type: 'divider',
        },
        {
          key: 'export',
          icon: <ExportOutlined />,
          label: `Export${bulkNames.length > 1 ? ` (${bulkNames.length})` : ''}`,
          onClick: () => handleExport(bulkNames),
        },
        {
          type: 'divider',
        },
        {
          key: 'delete',
          icon: <DeleteOutlined />,
          label: `Delete${bulkNames.length > 1 ? ` (${bulkNames.length})` : ''}`,
          danger: true,
          onClick: () => handleBulkDelete(bulkNames),
        },
      ];
    }

    return [
      {
        key: 'toggle',
        icon: enabled ? <PoweroffOutlined /> : <CheckOutlined />,
        label: enabled ? 'Disable' : 'Enable',
        onClick: () => handleToggle(name, !enabled),
      },
      {
        key: 'rename',
        icon: <EditOutlined />,
        label: 'Rename',
        onClick: () => {
          setRenameTarget(name);
          setNewName(name);
          setRenameModalVisible(true);
        },
      },
      {
        type: 'divider',
      },
      {
        key: 'export',
        icon: <ExportOutlined />,
        label: `Export${bulkNames.length > 1 ? ` (${bulkNames.length})` : ''}`,
        onClick: () => handleExport(bulkNames),
      },
      {
        type: 'divider',
      },
      {
        key: 'delete',
        icon: <DeleteOutlined />,
        label: `Delete${bulkNames.length > 1 ? ` (${bulkNames.length})` : ''}`,
        danger: true,
        onClick: () => handleBulkDelete(bulkNames),
      },
    ];
  };

  const handleRuleDrop = useCallback(
    async (targetName: string, position: 'before' | 'after') => {
      if (!draggedRuleName || draggedRuleName === targetName) {
        setDraggedRuleName(null);
        setDropTarget(null);
        stopAutoScroll();
        return;
      }

      const draggedFolder = getTopFolderPrefix(draggedRuleName);
      const targetFolder = getTopFolderPrefix(targetName);

      if (draggedFolder && draggedFolder === targetFolder) {
        setDraggedRuleName(null);
        setDropTarget(null);
        stopAutoScroll();
        return;
      }

      let reordered = [...rules];

      if (draggedFolder) {
        const folderPrefix = draggedFolder + '/';
        const group = reordered.filter((r) => r.name.startsWith(folderPrefix));
        const rest = reordered.filter((r) => !r.name.startsWith(folderPrefix));

        let insertIdx: number;
        if (targetFolder) {
          const targetPrefix = targetFolder + '/';
          const firstTargetIdx = rest.findIndex((r) => r.name.startsWith(targetPrefix));
          const lastTargetIdx = rest.reduce(
            (last, r, i) => (r.name.startsWith(targetPrefix) ? i : last),
            -1
          );
          insertIdx = position === 'before' ? firstTargetIdx : lastTargetIdx + 1;
        } else {
          const targetIdx = rest.findIndex((r) => r.name === targetName);
          if (targetIdx === -1) {
            setDraggedRuleName(null);
            setDropTarget(null);
            stopAutoScroll();
            return;
          }
          insertIdx = position === 'before' ? targetIdx : targetIdx + 1;
        }

        if (insertIdx === -1) insertIdx = rest.length;
        rest.splice(insertIdx, 0, ...group);
        reordered = rest;
      } else {
        const fromIndex = reordered.findIndex((r) => r.name === draggedRuleName);
        const targetIndex = reordered.findIndex((r) => r.name === targetName);
        if (fromIndex === -1 || targetIndex === -1) {
          setDraggedRuleName(null);
          setDropTarget(null);
          stopAutoScroll();
          return;
        }

        const [moved] = reordered.splice(fromIndex, 1);

        let insertIdx: number;
        if (targetFolder) {
          const targetPrefix = targetFolder + '/';
          const remaining = reordered;
          const firstTargetIdx = remaining.findIndex((r) => r.name.startsWith(targetPrefix));
          const lastTargetIdx = remaining.reduce(
            (last, r, i) => (r.name.startsWith(targetPrefix) ? i : last),
            -1
          );
          insertIdx = position === 'before' ? firstTargetIdx : lastTargetIdx + 1;
        } else {
          const adjustedTargetIndex =
            fromIndex < targetIndex ? targetIndex - 1 : targetIndex;
          insertIdx = position === 'before' ? adjustedTargetIndex : adjustedTargetIndex + 1;
        }

        if (insertIdx === -1) insertIdx = reordered.length;
        reordered.splice(insertIdx, 0, moved);
      }

      setDraggedRuleName(null);
      setDropTarget(null);
      stopAutoScroll();

      const success = await reorderRules(reordered.map((r) => r.name));
      if (success) {
        message.success('Rule order updated');
      }
    },
    [draggedRuleName, reorderRules, rules, stopAutoScroll]
  );

  const handleListKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) {
        return;
      }

      if (visibleRuleNames.length === 0) {
        return;
      }

      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') {
        return;
      }

      event.preventDefault();

      const currentIndex = selectedRuleName ? visibleRuleNames.indexOf(selectedRuleName) : -1;

      const fallbackIndex = event.key === 'ArrowDown' ? 0 : visibleRuleNames.length - 1;
      const nextIndex =
        currentIndex === -1
          ? fallbackIndex
          : Math.min(
              visibleRuleNames.length - 1,
              Math.max(0, currentIndex + (event.key === 'ArrowDown' ? 1 : -1))
            );

      const nextName = visibleRuleNames[nextIndex];
      if (!nextName || nextName === selectedRuleName) {
        return;
      }

      setSelectedRules([]);
      lastClickedIndexRef.current = nextIndex;
      void selectRule(nextName);
    },
    [visibleRuleNames, selectedRuleName, selectRule]
  );

  const prevSelectedRef = useRef<string | null>(null);
  useEffect(() => {
    if (!selectedRuleName || selectedRuleName === prevSelectedRef.current) {
      prevSelectedRef.current = selectedRuleName;
      return;
    }
    prevSelectedRef.current = selectedRuleName;

    const selectedItem = listContainerRef.current?.querySelector<HTMLElement>(
      `[data-rule-name="${CSS.escape(selectedRuleName)}"]`
    );
    selectedItem?.scrollIntoView({ block: 'nearest' });
  }, [selectedRuleName, filteredRules]);

  const handleToggleFolder = useCallback((path: string) => {
    setExpandedFolders((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return Array.from(next);
    });
  }, []);

  const renderTreeNodes = (folder: RuleTreeFolderNode, depth: number): ReactNode[] => {
    const nodes: ReactNode[] = [];

    for (const child of folder.children) {
      if (child.type === 'folder') {
        const expanded = expandedFolderSet.has(child.path);
        nodes.push(
          <div key={`folder-wrapper-${child.path}`}>
            <div
              className={`${styles.item} ${styles.folderItem}`}
              style={{ paddingLeft: 12 + depth * 16 }}
              onClick={() => {
                listContainerRef.current?.focus();
                handleToggleFolder(child.path);
              }}
              onDoubleClick={(e) => {
                e.preventDefault();
                handleToggleFolder(child.path);
              }}
              data-testid="rule-folder-item"
              data-folder-path={child.path}
              data-folder-expanded={expanded ? 'true' : 'false'}
            >
              <div className={styles.itemContent}>
                <span className={styles.folderCaret} aria-hidden="true">
                  {expanded ? <CaretDownOutlined /> : <CaretRightOutlined />}
                </span>
                {expanded ? (
                  <FolderOpenOutlined className={styles.folderIcon} />
                ) : (
                  <FolderOutlined className={styles.folderIcon} />
                )}
                <span className={styles.itemName} title={child.path}>
                  {child.name}
                </span>
              </div>
            </div>
            {expanded && renderTreeNodes(child, depth + 1)}
          </div>
        );
        continue;
      }

      const rule = child.rule;
      const isSelected = selectedRuleName === rule.name;
      const hasChanges = hasUnsavedChanges(rule.name);

      nodes.push(
        <Dropdown
          key={rule.name}
          menu={{ items: getContextMenuItems(rule.name, rule.enabled) }}
          trigger={['contextMenu']}
        >
          <div
            id={getRuleItemId(rule.name)}
            className={`${styles.item} ${isSelected ? styles.selected : ''} ${selectedRules.includes(rule.name) ? styles.multiSelected : ''}`}
            role="option"
            aria-selected={isSelected}
            draggable={!isGroupMode && sortMode === 'manual'}
            onClick={(e) => {
              listContainerRef.current?.focus();
              handleSelect(rule.name, e);
            }}
            onDoubleClick={() => handleToggle(rule.name, !rule.enabled)}
            onDragStart={() => {
              if (isGroupMode || sortMode !== 'manual') return;
              setDraggedRuleName(rule.name);
            }}
            onDragEnd={() => {
              setDraggedRuleName(null);
              setDropTarget(null);
              stopAutoScroll();
            }}
            onDragOver={(e) => {
              if (sortMode !== 'manual' || !draggedRuleName || draggedRuleName === rule.name) {
                return;
              }
              e.preventDefault();
              updateAutoScroll(e.clientY);
              const rect = e.currentTarget.getBoundingClientRect();
              const position = e.clientY - rect.top < rect.height / 2 ? 'before' : 'after';
              if (dropTarget?.name !== rule.name || dropTarget.position !== position) {
                setDropTarget({ name: rule.name, position });
              }
            }}
            onDrop={(e) => {
              if (sortMode !== 'manual') return;
              e.preventDefault();
              stopAutoScroll();
              const rect = e.currentTarget.getBoundingClientRect();
              const position = e.clientY - rect.top < rect.height / 2 ? 'before' : 'after';
              void handleRuleDrop(rule.name, position);
            }}
            style={{ paddingLeft: 12 + depth * 16 }}
            data-testid="rule-item"
            data-rule-name={rule.name}
            data-rule-enabled={rule.enabled ? 'true' : 'false'}
            data-dragging={draggedRuleName === rule.name ? 'true' : 'false'}
            data-drop-position={dropTarget?.name === rule.name ? dropTarget.position : undefined}
          >
            <div className={styles.itemContent}>
              {!isGroupMode && sortMode === 'manual' && (
                <Tooltip title="Drag to reorder">
                  <HolderOutlined className={styles.dragHandle} />
                </Tooltip>
              )}
              <span className={styles.itemName} title={rule.name}>
                {child.label}
              </span>
              <div className={styles.itemMeta}>
                {hasChanges && (
                  <Tooltip title="Unsaved changes">
                    <span className={styles.unsavedDot} />
                  </Tooltip>
                )}
                {rule.enabled && (
                  <Tooltip title="Enabled">
                    <CheckOutlined className={styles.enabledIcon} />
                  </Tooltip>
                )}
              </div>
            </div>
            <div
              className={styles.itemExtra}
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => e.stopPropagation()}
              onMouseDown={(e) => e.stopPropagation()}
            >
              <Switch
                size="small"
                checked={rule.enabled}
                onChange={(checked, e) => {
                  e.stopPropagation();
                  handleToggle(rule.name, checked);
                }}
              />
            </div>
          </div>
        </Dropdown>
      );
    }

    return nodes;
  };

  return (
    <div className={styles.container} data-testid="rules-list">
      {showGroupSwitcher && (
        <div style={{ padding: '6px 8px', borderBottom: '1px solid var(--ant-color-border-secondary, #f0f0f0)' }}>
          <Select
            size="small"
            showSearch
            value={activeGroupId ?? '__my__'}
            onChange={handleGroupChange}
            options={groupSwitcherOptions}
            style={{ width: '100%' }}
            suffixIcon={<SwapOutlined />}
            popupMatchSelectWidth={true}
            filterOption={(input, option) => {
              const text = (option as { searchText?: string })?.searchText ?? '';
              return text.toLowerCase().includes(input.toLowerCase());
            }}
          />
        </div>
      )}
      <div className={styles.header}>
        <span className={styles.headerTitle}>Rules</span>
        <div className={styles.headerActions}>
          {canEdit && (
            <Tooltip title="New Rule">
              <Button
                type="text"
                size="small"
                icon={<PlusOutlined />}
                onClick={() => setCreateModalVisible(true)}
                data-testid="rule-new-button"
              />
            </Tooltip>
          )}
          <Tooltip title="Refresh">
            <Button
              type="text"
              size="small"
              icon={<ReloadOutlined />}
              onClick={() => fetchRules()}
              data-testid="rule-refresh-button"
            />
          </Tooltip>
          {!isGroupMode && (
            <ImportBifrostButton
              expectedType="rules"
              onImportSuccess={handleImportSuccess}
              buttonText=""
              buttonType="text"
              size="small"
              testId="rule-import-button"
            />
          )}
        </div>
      </div>
      <div className={styles.searchBox}>
        <Input
          size="small"
          placeholder="Search rules..."
          prefix={<SearchOutlined style={{ color: '#999' }} />}
          value={searchKeyword}
          onChange={(e) => setSearchKeyword(e.target.value)}
          allowClear
          data-testid="rule-search-input"
        />
        <Select
          size="small"
          value={sortMode}
          onChange={(value: RuleSortMode) => setSortMode(value)}
          options={ruleSortOptions}
          className={styles.sortControl}
          popupMatchSelectWidth={false}
          data-testid="rule-sort-select"
        />
      </div>

      <div className={styles.listWrapper}>
        {showLoadingOverlay && (
          <div className={styles.loadingOverlay}>
            <Spin size="small" />
          </div>
        )}
        <div
          ref={listContainerRef}
          className={styles.listContainer}
        tabIndex={0}
        role="listbox"
        aria-label="Rules list"
        aria-activedescendant={selectedRuleName ? getRuleItemId(selectedRuleName) : undefined}
        onKeyDown={handleListKeyDown}
        onDragOver={(e) => {
          if (sortMode !== 'manual' || !draggedRuleName) {
            stopAutoScroll();
            return;
          }
          updateAutoScroll(e.clientY);
        }}
        onDragLeave={(e) => {
          const nextTarget = e.relatedTarget;
          if (
            nextTarget instanceof Node &&
            listContainerRef.current?.contains(nextTarget)
          ) {
            return;
          }
          stopAutoScroll();
        }}
        onDrop={() => {
          stopAutoScroll();
        }}
      >
        {loading && rules.length === 0 ? (
          <div className={styles.loading}>
            <Spin size="small" />
          </div>
        ) : (
          <div className={styles.list}>
            {renderTreeNodes(ruleTree, 0)}
            {filteredRules.length === 0 && !loading && (
              <div className={styles.empty}>
                {searchKeyword ? 'No matching rules' : 'No rules yet'}
              </div>
            )}
          </div>
        )}
      </div>
      </div>

      <div className={styles.footer}>
        <span className={styles.stats}>
          {rules.length} rules, {rules.filter((r) => r.enabled).length} enabled
          {isReadOnlyGroup && ' (Read-only)'}
        </span>
      </div>

      <Modal
        title="New Rule"
        open={createModalVisible}
        onCancel={() => {
          setCreateModalVisible(false);
          setNewRuleName('');
        }}
        onOk={handleCreate}
        okText="Create"
        cancelText="Cancel"
      >
        <Input
          placeholder="Rule name"
          value={newRuleName}
          onChange={(e) => setNewRuleName(e.target.value)}
          onPressEnter={handleCreate}
          autoFocus
        />
      </Modal>

      <Modal
        title="Rename Rule"
        open={renameModalVisible}
        onCancel={() => {
          setRenameModalVisible(false);
          setRenameTarget(null);
          setNewName('');
        }}
        onOk={handleRename}
        okText="Rename"
        cancelText="Cancel"
      >
        <Input
          placeholder="New name"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onPressEnter={handleRename}
          autoFocus
        />
      </Modal>
    </div>
  );
}
