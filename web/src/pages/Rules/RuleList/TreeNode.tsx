import { useState, useCallback } from 'react';
import { Tooltip, Switch, Dropdown } from 'antd';
import {
  FolderOutlined,
  FolderOpenOutlined,
  FileTextOutlined,
  CheckOutlined,
  HolderOutlined,
} from '@ant-design/icons';
import type { MenuProps } from 'antd';
import type { TreeNode } from './treeUtils';
import styles from './index.module.css';

interface TreeNodeProps {
  node: TreeNode;
  selectedRuleName: string | null;
  selectedRules: string[];
  editingContent: Record<string, string>;
  hasUnsavedChanges: (name: string) => boolean;
  isGroupMode: boolean;
  sortMode: 'manual' | 'updated_desc' | 'name_asc';
  draggedRuleName: string | null;
  dropTarget: { name: string; position: 'before' | 'after' } | null;
  onSelect: (name: string, e: React.MouseEvent) => void;
  onToggle: (name: string, enabled: boolean) => void;
  getContextMenuItems: (name: string, enabled: boolean) => MenuProps['items'];
  onDragStart: (name: string) => void;
  onDragEnd: () => void;
  onDragOver: (name: string, e: React.DragEvent) => void;
  onDrop: (name: string, e: React.DragEvent) => void;
  getRuleItemId: (name: string) => string;
  indentLevel?: number;
}

export default function TreeNodeComponent({
  node,
  selectedRuleName,
  selectedRules,
  editingContent,
  hasUnsavedChanges,
  isGroupMode,
  sortMode,
  draggedRuleName,
  dropTarget,
  onSelect,
  onToggle,
  getContextMenuItems,
  onDragStart,
  onDragEnd,
  onDragOver,
  onDrop,
  getRuleItemId,
  indentLevel = 0,
}: TreeNodeProps) {
  const [isExpanded, setIsExpanded] = useState(true);

  const toggleExpand = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    setIsExpanded(!isExpanded);
  }, [isExpanded]);

  if (node.isDirectory) {
    return (
      <div className={styles.treeDirectory}>
        <div
          className={`${styles.item} ${styles.directoryItem}`}
          style={{ paddingLeft: `${12 + indentLevel * 20}px` }}
          onClick={toggleExpand}
        >
          <div className={styles.itemContent}>
            <span className={styles.expandIcon} onClick={toggleExpand}>
              {isExpanded ? <FolderOpenOutlined /> : <FolderOutlined />}
            </span>
            <span className={styles.itemName} title={node.name}>
              {node.name}
            </span>
          </div>
        </div>
        {isExpanded && node.children?.map((child) => (
          <TreeNodeComponent
            key={child.fullPath}
            node={child}
            selectedRuleName={selectedRuleName}
            selectedRules={selectedRules}
            editingContent={editingContent}
            hasUnsavedChanges={hasUnsavedChanges}
            isGroupMode={isGroupMode}
            sortMode={sortMode}
            draggedRuleName={draggedRuleName}
            dropTarget={dropTarget}
            onSelect={onSelect}
            onToggle={onToggle}
            getContextMenuItems={getContextMenuItems}
            onDragStart={onDragStart}
            onDragEnd={onDragEnd}
            onDragOver={onDragOver}
            onDrop={onDrop}
            getRuleItemId={getRuleItemId}
            indentLevel={indentLevel + 1}
          />
        ))}
      </div>
    );
  }

  if (!node.rule) return null;

  const rule = node.rule;
  const isSelected = selectedRuleName === rule.name;
  const hasChanges = hasUnsavedChanges(rule.name) || editingContent[rule.name] !== undefined;

  return (
    <Dropdown
      key={rule.name}
      menu={{ items: getContextMenuItems(rule.name, rule.enabled) }}
      trigger={['contextMenu']}
    >
      <div
        id={getRuleItemId(rule.name)}
        className={`${styles.item} ${isSelected ? styles.selected : ''} ${selectedRules.includes(rule.name) ? styles.multiSelected : ''}`}
        style={{ paddingLeft: `${12 + indentLevel * 20}px` }}
        role="option"
        aria-selected={isSelected}
        draggable={!isGroupMode && sortMode === 'manual'}
        onClick={(e) => {
          onSelect(rule.name, e);
        }}
        onDoubleClick={() => onToggle(rule.name, !rule.enabled)}
        onDragStart={() => {
          if (isGroupMode || sortMode !== 'manual') return;
          onDragStart(rule.name);
        }}
        onDragEnd={onDragEnd}
        onDragOver={(e) => onDragOver(rule.name, e)}
        onDrop={(e) => onDrop(rule.name, e)}
        data-testid="rule-item"
        data-rule-name={rule.name}
        data-rule-enabled={rule.enabled ? 'true' : 'false'}
        data-dragging={draggedRuleName === rule.name ? 'true' : 'false'}
        data-drop-position={
          dropTarget?.name === rule.name ? dropTarget.position : undefined
        }
      >
        <div className={styles.itemContent}>
          {!isGroupMode && sortMode === 'manual' && (
            <Tooltip title="Drag to reorder">
              <HolderOutlined className={styles.dragHandle} />
            </Tooltip>
          )}
          <span className={styles.expandIcon}>
            <FileTextOutlined />
          </span>
          <span className={styles.itemName} title={rule.name}>
            {node.name}
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
              onToggle(rule.name, checked);
            }}
          />
        </div>
      </div>
    </Dropdown>
  );
}
