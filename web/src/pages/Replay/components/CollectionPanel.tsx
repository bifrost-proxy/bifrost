import { useCallback, useState, useMemo, useEffect, type CSSProperties } from "react";
import { Input, Tree, Button, Dropdown, Empty, Typography, Tag, theme, Modal, message } from "antd";
import type { TreeProps } from "antd";
import {
  SearchOutlined,
  PlusOutlined,
  FolderOutlined,
  FolderOpenOutlined,
  FileOutlined,
  MoreOutlined,
  DeleteOutlined,
  EditOutlined,
  FolderAddOutlined,
  ExportOutlined,
} from "@ant-design/icons";
import type { DataNode } from "antd/es/tree";
import { useReplayStore } from "../../../stores/useReplayStore";
import type { ReplayRequestSummary, ReplayGroup } from "../../../types";
import { ImportBifrostButton } from "../../../components/ImportBifrostButton";
import { useExportBifrost } from "../../../hooks/useExportBifrost";
import { isMacDesktopShell } from "../../../runtime";

const { Text } = Typography;

const METHOD_COLORS: Record<string, string> = {
  GET: "#52c41a",
  POST: "#1890ff",
  PUT: "#fa8c16",
  DELETE: "#f5222d",
  PATCH: "#722ed1",
  OPTIONS: "#8c8c8c",
  HEAD: "#13c2c2",
};

function truncateUrl(url: string, maxLength = 25): string {
  try {
    const urlObj = new URL(url);
    const path = urlObj.pathname + urlObj.search;
    if (path.length <= maxLength) return path;
    return path.substring(0, maxLength) + '...';
  } catch {
    if (url.length <= maxLength) return url;
    return url.substring(0, maxLength) + '...';
  }
}

function highlightText(text: string, keyword: string, color: string): React.ReactNode {
  if (!keyword) return text;
  const lowerText = text.toLowerCase();
  const lowerKeyword = keyword.toLowerCase();
  const index = lowerText.indexOf(lowerKeyword);
  if (index === -1) return text;
  
  const before = text.substring(0, index);
  const match = text.substring(index, index + keyword.length);
  const after = text.substring(index + keyword.length);
  
  return (
    <>
      {before}
      <span style={{ backgroundColor: color, borderRadius: 2 }}>{match}</span>
      {after}
    </>
  );
}

function isActionButtonClick(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return !!target.closest(".tree-node-more-btn");
}

export default function CollectionPanel() {
  const { token } = theme.useToken();
  const macDesktopShell = isMacDesktopShell();
  const {
    savedRequests,
    currentRequest,
    groups,
    selectRequest,
    deleteRequest,
    createNewRequest,
    loadGroups,
    createGroup,
    deleteGroup,
    updateGroup,
    moveRequest,
    uiState,
    updateUIState,
  } = useReplayStore();

  const [searchText, setSearchText] = useState("");
  const expandedKeys = uiState.collectionExpandedKeys;
  const setExpandedKeys = useCallback((keys: string[]) => {
    updateUIState({ collectionExpandedKeys: keys });
  }, [updateUIState]);
  const { exportFile } = useExportBifrost();
  const [newGroupModalVisible, setNewGroupModalVisible] = useState(false);
  const [newGroupName, setNewGroupName] = useState("");
  const [editGroupId, setEditGroupId] = useState<string | null>(null);
  const [editGroupName, setEditGroupName] = useState("");

  useEffect(() => {
    const { uiState: currentUIState } = useReplayStore.getState();
    const currentKeys = currentUIState.collectionExpandedKeys;
    const keysToAdd: string[] = [];
    
    if (groups.length > 0) {
      const groupKeys = groups.map(g => `group-${g.id}`);
      keysToAdd.push(...groupKeys.filter(k => !currentKeys.includes(k)));
    }
    
    if (!currentKeys.includes('ungrouped')) {
      keysToAdd.push('ungrouped');
    }
    
    if (keysToAdd.length > 0) {
      updateUIState({ collectionExpandedKeys: [...currentKeys, ...keysToAdd] });
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groups.map(g => g.id).join(','), updateUIState]);
  
  useEffect(() => {
    const { uiState: current } = useReplayStore.getState();
    if (!current.collectionExpandedKeys.includes('ungrouped')) {
      updateUIState({ collectionExpandedKeys: [...current.collectionExpandedKeys, 'ungrouped'] });
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleSelectRequest = useCallback((item: ReplayRequestSummary) => {
    selectRequest(item);
  }, [selectRequest]);

  const handleDeleteRequest = useCallback(async (id: string) => {
    await deleteRequest(id);
  }, [deleteRequest]);

  const handleCreateGroup = useCallback(async () => {
    if (!newGroupName.trim()) {
      message.warning('Please enter a group name');
      return;
    }
    const success = await createGroup(newGroupName.trim());
    if (success) {
      setNewGroupModalVisible(false);
      setNewGroupName("");
    }
  }, [createGroup, newGroupName]);

  const handleDeleteGroup = useCallback(async (groupId: string) => {
    await deleteGroup(groupId);
  }, [deleteGroup]);

  const handleUpdateGroup = useCallback(async () => {
    if (!editGroupId || !editGroupName.trim()) return;
    const success = await updateGroup(editGroupId, editGroupName.trim());
    if (success) {
      setEditGroupId(null);
      setEditGroupName("");
    }
  }, [editGroupId, editGroupName, updateGroup]);

  const handleMoveRequest = useCallback(async (requestId: string, groupId: string | null) => {
    await moveRequest(requestId, groupId);
  }, [moveRequest]);

  const handleExportRequest = useCallback(async (requestIds: string[]) => {
    if (requestIds.length === 0) return;
    await exportFile("template", { request_ids: requestIds });
  }, [exportFile]);

  const handleExportGroup = useCallback(async (groupId: string) => {
    await exportFile("template", { group_ids: [groupId] });
  }, [exportFile]);

  const handleExportAll = useCallback(async () => {
    const allRequestIds = savedRequests.map(r => r.id);
    if (allRequestIds.length === 0) return;
    await exportFile("template", { request_ids: allRequestIds });
  }, [savedRequests, exportFile]);

  const handleImportSuccess = useCallback(async () => {
    await loadGroups();
    await useReplayStore.getState().loadSavedRequests();
  }, [loadGroups]);

  const reorderGroups = useCallback(async (reorderedGroups: ReplayGroup[]) => {
    for (let i = 0; i < reorderedGroups.length; i++) {
      if (reorderedGroups[i].sort_order !== i) {
        await updateGroup(reorderedGroups[i].id, reorderedGroups[i].name);
      }
    }
    await loadGroups();
  }, [updateGroup, loadGroups]);

  const findGroupIdForRequest = useCallback((requestId: string): string | null => {
    const request = savedRequests.find(r => r.id === requestId);
    return request?.group_id || null;
  }, [savedRequests]);

  const filteredRequests = useMemo(() => {
    if (!searchText) return savedRequests;
    const lower = searchText.toLowerCase();
    return savedRequests.filter(r =>
      r.name?.toLowerCase().includes(lower) ||
      r.url.toLowerCase().includes(lower) ||
      r.method.toLowerCase().includes(lower)
    );
  }, [savedRequests, searchText]);

  const requestsByGroup = useMemo(() => {
    const grouped: Record<string, ReplayRequestSummary[]> = {
      ungrouped: [],
    };
    groups.forEach(g => {
      grouped[g.id] = [];
    });
    
    filteredRequests.forEach(req => {
      if (req.group_id && grouped[req.group_id]) {
        grouped[req.group_id].push(req);
      } else {
        grouped.ungrouped.push(req);
      }
    });
    
    return grouped;
  }, [filteredRequests, groups]);

  const styles: Record<string, CSSProperties> = useMemo(() => ({
    container: {
      display: 'flex',
      flexDirection: 'column',
      height: '100%',
      overflow: 'hidden',
      backgroundColor: macDesktopShell ? 'transparent' : token.colorBgLayout,
    },
    header: {
      display: 'flex',
      justifyContent: 'space-between',
      alignItems: 'center',
      padding: '8px 12px',
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
      flexShrink: 0,
    },
    headerTitle: {
      fontSize: 13,
      fontWeight: 600,
      color: token.colorText,
    },
    headerActions: {
      display: 'flex',
      gap: 4,
    },
    searchBox: {
      padding: '8px 12px',
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
      flexShrink: 0,
    },
    content: {
      flex: 1,
      overflowY: 'auto',
      overflowX: 'hidden',
      padding: '4px 0',
    },
    folderTitle: {
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      fontWeight: 500,
      fontSize: 12,
      flex: 1,
    },
    treeNode: {
      display: 'flex',
      alignItems: 'center',
      gap: 6,
      padding: '4px 8px',
      borderRadius: 4,
      cursor: 'pointer',
      minWidth: 0,
    },
    treeNodeActive: {
      backgroundColor: token.colorPrimaryBg,
    },
    methodBadge: {
      fontSize: 10,
      fontWeight: 600,
      flexShrink: 0,
    },
    nodeName: {
      flex: 1,
      overflow: 'hidden',
      textOverflow: 'ellipsis',
      whiteSpace: 'nowrap',
      fontSize: 12,
    },
    countTag: {
      fontSize: 10,
      padding: '0 4px',
      lineHeight: '16px',
      margin: 0,
    },
    empty: {
      padding: '40px 20px',
    },
    groupHeader: {
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'space-between',
      width: '100%',
    },
  }), [macDesktopShell, token]);

  const buildRequestNode = useCallback((req: ReplayRequestSummary, groupId: string | null): DataNode => {
    const displayName = req.name || truncateUrl(req.url);
    const highlightColor = token.colorWarningBg;
    
    return {
      key: `req-${req.id}`,
      title: (
        <div
          style={{
            ...styles.treeNode,
            ...(currentRequest?.id === req.id ? styles.treeNodeActive : {}),
          }}
          onClick={(e) => {
            if (isActionButtonClick(e.target)) {
              return;
            }
            handleSelectRequest(req);
          }}
          data-testid="replay-request-node"
          data-request-id={req.id}
        >
          <FileOutlined style={{ fontSize: 12, flexShrink: 0, marginRight: 6 }} />
          <span style={{ ...styles.methodBadge, color: METHOD_COLORS[req.method] || '#8c8c8c' }}>
            {searchText ? highlightText(req.method, searchText, highlightColor) : req.method}
          </span>
          <span style={styles.nodeName}>
            {searchText ? highlightText(displayName, searchText, highlightColor) : displayName}
          </span>
          <Dropdown
            menu={{
              items: [
                { key: 'export', label: 'Export', icon: <ExportOutlined /> },
                ...(groups.length > 0 ? [
                  { type: 'divider' as const },
                  {
                    key: 'move',
                    label: 'Move to...',
                    children: [
                      ...(groupId ? [{ key: 'move-ungrouped', label: 'Ungrouped' }] : []),
                      ...groups
                        .filter(g => g.id !== groupId)
                        .map(g => ({ key: `move-${g.id}`, label: g.name })),
                    ],
                  },
                ] : []),
                { type: 'divider' as const },
                { key: 'delete', label: 'Delete', icon: <DeleteOutlined />, danger: true },
              ],
              onClick: ({ key }) => {
                if (key === 'export') {
                  handleExportRequest([req.id]);
                } else if (key === 'delete') {
                  handleDeleteRequest(req.id);
                } else if (key === 'move-ungrouped') {
                  handleMoveRequest(req.id, null);
                } else if (key.startsWith('move-')) {
                  const targetGroupId = key.replace('move-', '');
                  handleMoveRequest(req.id, targetGroupId);
                }
              },
            }}
            trigger={['click']}
          >
            <Button
              type="text"
              size="small"
              icon={<MoreOutlined />}
              onClick={(e) => e.stopPropagation()}
              onMouseDown={(e) => e.stopPropagation()}
              className="tree-node-more-btn"
              data-testid="replay-request-actions-button"
            />
          </Dropdown>
        </div>
      ),
      isLeaf: true,
    };
  }, [currentRequest, groups, handleSelectRequest, handleDeleteRequest, handleMoveRequest, handleExportRequest, styles, searchText, token]);

  const treeData: DataNode[] = useMemo(() => {
    const nodes: DataNode[] = [];

    groups.forEach(group => {
      const groupRequests = requestsByGroup[group.id] || [];
      if (searchText && groupRequests.length === 0) return;
      
      const isExpanded = expandedKeys.includes(`group-${group.id}`);
      nodes.push({
        key: `group-${group.id}`,
        title: (
          <div style={styles.groupHeader} data-group-id={group.id}>
            <div style={styles.folderTitle}>
              {isExpanded ? (
                <FolderOpenOutlined style={{ fontSize: 12, marginRight: 6 }} />
              ) : (
                <FolderOutlined style={{ fontSize: 12, marginRight: 6 }} />
              )}
              <span>{group.name}</span>
              <Tag style={styles.countTag}>{groupRequests.length}</Tag>
            </div>
            <Dropdown
              menu={{
                items: [
                  { key: 'export', label: `Export (${groupRequests.length})`, icon: <ExportOutlined />, disabled: groupRequests.length === 0 },
                  { type: 'divider' },
                  { key: 'rename', label: 'Rename', icon: <EditOutlined /> },
                  { type: 'divider' },
                  { key: 'delete', label: 'Delete', icon: <DeleteOutlined />, danger: true },
                ],
                onClick: ({ key }) => {
                  if (key === 'export') {
                    handleExportGroup(group.id);
                  } else if (key === 'rename') {
                    setEditGroupId(group.id);
                    setEditGroupName(group.name);
                  } else if (key === 'delete') {
                    handleDeleteGroup(group.id);
                  }
                },
              }}
              trigger={['click']}
            >
              <Button
                type="text"
                size="small"
                icon={<MoreOutlined />}
                onClick={(e) => e.stopPropagation()}
                onMouseDown={(e) => e.stopPropagation()}
                className="tree-node-more-btn"
                data-testid="replay-group-actions-button"
              />
            </Dropdown>
          </div>
        ),
        children: groupRequests.map(req => buildRequestNode(req, group.id)),
      });
    });

    const ungroupedRequests = requestsByGroup.ungrouped || [];
    if (ungroupedRequests.length > 0) {
      const isExpanded = expandedKeys.includes('ungrouped');
      nodes.push({
        key: 'ungrouped',
        title: (
          <div style={styles.folderTitle}>
            {isExpanded ? (
              <FolderOpenOutlined style={{ fontSize: 12, marginRight: 6 }} />
            ) : (
              <FolderOutlined style={{ fontSize: 12, marginRight: 6 }} />
            )}
            <span>Ungrouped</span>
            <Tag style={styles.countTag}>{ungroupedRequests.length}</Tag>
          </div>
        ),
        children: ungroupedRequests.map(req => buildRequestNode(req, null)),
      });
    }

    return nodes;
  }, [groups, requestsByGroup, buildRequestNode, styles, handleDeleteGroup, handleExportGroup, searchText, expandedKeys]);

  const handleDrop: TreeProps<DataNode>['onDrop'] = useCallback(async (info: Parameters<NonNullable<TreeProps<DataNode>['onDrop']>>[0]) => {
    const dragKey = info.dragNode.key as string;
    const dropKey = info.node.key as string;
    const dropPos = info.node.pos.split('-');
    const dropPosition = info.dropPosition - Number(dropPos[dropPos.length - 1]);

    if (dragKey.startsWith('req-')) {
      const requestId = dragKey.replace('req-', '');
      
      if (dropKey.startsWith('group-')) {
        const targetGroupId = dropKey.replace('group-', '');
        await handleMoveRequest(requestId, targetGroupId);
      } else if (dropKey === 'ungrouped') {
        await handleMoveRequest(requestId, null);
      } else if (dropKey.startsWith('req-')) {
        const parentNode = treeData.find(node => 
          node.children?.some(child => child.key === dropKey)
        );
        const parentKey = parentNode?.key as string | undefined;
        let dropNodeGroupId: string | null = null;
        
        if (parentKey?.startsWith('group-')) {
          dropNodeGroupId = parentKey.replace('group-', '');
        }
        
        const currentGroupId = findGroupIdForRequest(requestId);
        if (currentGroupId !== dropNodeGroupId) {
          await handleMoveRequest(requestId, dropNodeGroupId);
        }
      }
    } else if (dragKey.startsWith('group-')) {
      const dragGroupId = dragKey.replace('group-', '');
      const dragGroup = groups.find(g => g.id === dragGroupId);
      if (!dragGroup) return;

      if (dropKey.startsWith('group-') || dropKey === 'ungrouped') {
        const newGroups = [...groups];
        const dragIndex = newGroups.findIndex(g => g.id === dragGroupId);
        
        if (dropKey === 'ungrouped') {
          newGroups.splice(dragIndex, 1);
          newGroups.push(dragGroup);
        } else {
          const dropGroupId = dropKey.replace('group-', '');
          const dropIndex = newGroups.findIndex(g => g.id === dropGroupId);
          
          newGroups.splice(dragIndex, 1);
          const insertIndex = dropPosition === -1 ? dropIndex : dropIndex + 1;
          newGroups.splice(insertIndex > dragIndex ? insertIndex - 1 : insertIndex, 0, dragGroup);
        }

        await reorderGroups(newGroups);
      }
    }
  }, [handleMoveRequest, groups, treeData, findGroupIdForRequest, reorderGroups]);

  const allowDrop: TreeProps<DataNode>['allowDrop'] = useCallback(({ dragNode, dropNode, dropPosition }: { dragNode: DataNode; dropNode: DataNode; dropPosition: number }) => {
    const dragKey = dragNode.key as string;
    const dropKey = dropNode.key as string;

    if (dragKey.startsWith('req-')) {
      if (dropKey.startsWith('group-') || dropKey === 'ungrouped') {
        return true;
      }
      if (dropKey.startsWith('req-')) {
        return dropPosition === 0;
      }
      return false;
    }

    if (dragKey.startsWith('group-')) {
      if (dropKey.startsWith('group-')) {
        return dropPosition !== 0;
      }
      if (dropKey === 'ungrouped') {
        return dropPosition === -1;
      }
      return false;
    }

    return false;
  }, []);

  return (
    <div style={styles.container} data-testid="replay-collection-panel">
      <div style={styles.header}>
        <span style={styles.headerTitle}>Collections</span>
        <div style={styles.headerActions}>
          <Button
            type="text"
            size="small"
            icon={<FolderAddOutlined />}
            onClick={() => setNewGroupModalVisible(true)}
            title="New Folder"
            data-testid="replay-new-group-button"
          />
          <Button
            type="text"
            size="small"
            icon={<PlusOutlined />}
            onClick={createNewRequest}
            title="New Request"
            data-testid="replay-new-request-button"
          />
          {savedRequests.length > 0 && (
            <Button
              type="text"
              size="small"
              icon={<ExportOutlined />}
              onClick={handleExportAll}
              title="Export All"
            />
          )}
          <ImportBifrostButton
            expectedType="template"
            onImportSuccess={handleImportSuccess}
            buttonText=""
            buttonType="text"
            size="small"
          />
        </div>
      </div>

      <div style={styles.searchBox}>
        <Input
          placeholder="Search..."
          prefix={<SearchOutlined />}
          value={searchText}
          onChange={(e) => setSearchText(e.target.value)}
          allowClear
          size="small"
          data-testid="replay-search-input"
        />
      </div>

      <div style={styles.content}>
        {(treeData.length > 0) ? (
          <Tree
            treeData={treeData}
            expandedKeys={expandedKeys}
            onExpand={(keys) => setExpandedKeys(keys as string[])}
            blockNode
            selectable={false}
            draggable={{
              icon: false,
              nodeDraggable: (node) => {
                const key = node.key as string;
                return key !== 'ungrouped';
              },
            }}
            allowDrop={allowDrop}
            onDrop={handleDrop}
            style={{ backgroundColor: 'transparent' }}
            data-testid="replay-collection-tree"
          />
        ) : (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={
              <Text type="secondary" style={{ fontSize: 12 }}>
                {searchText ? "No matching requests" : "No saved requests yet"}
              </Text>
            }
            style={styles.empty}
          >
            {!searchText && (
              <Button type="primary" size="small" onClick={createNewRequest}>
                New Request
              </Button>
            )}
          </Empty>
        )}
      </div>

      <Modal
        title="New Folder"
        open={newGroupModalVisible}
        onOk={handleCreateGroup}
        onCancel={() => {
          setNewGroupModalVisible(false);
          setNewGroupName("");
        }}
        okText="Create"
      >
        <Input
          placeholder="Folder name"
          value={newGroupName}
          onChange={(e) => setNewGroupName(e.target.value)}
          onPressEnter={handleCreateGroup}
          autoFocus
          data-testid="replay-group-name-input"
        />
      </Modal>

      <Modal
        title="Rename Folder"
        open={!!editGroupId}
        onOk={handleUpdateGroup}
        onCancel={() => {
          setEditGroupId(null);
          setEditGroupName("");
        }}
        okText="Save"
      >
        <Input
          placeholder="Folder name"
          value={editGroupName}
          onChange={(e) => setEditGroupName(e.target.value)}
          onPressEnter={handleUpdateGroup}
          autoFocus
          data-testid="replay-group-rename-input"
        />
      </Modal>
    </div>
  );
}
