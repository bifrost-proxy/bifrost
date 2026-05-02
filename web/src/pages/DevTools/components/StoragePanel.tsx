import { useCallback, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type UIEvent } from "react";
import { CheckOutlined, CloseOutlined, CopyOutlined, DeleteOutlined, EditOutlined, PlusOutlined } from "@ant-design/icons";
import { Button, Empty, Input, Space, Typography } from "antd";
import type { DebugStorageSnapshot } from "../../../api/devtools";
import { HighlightedText } from "./shared";
import { filterBySearch } from "./sharedUtils";

const { Text } = Typography;
const STORAGE_ROW_HEIGHT = 37;
const STORAGE_ROW_OVERSCAN = 8;

export function StorageView({
  storage,
  area,
  searchQuery,
  storageKey,
  storageValue,
  editingKey,
  saving,
  onKeyChange,
  onValueChange,
  onStartEdit,
  onStartAdd,
  onCancelEdit,
  onCopy,
  onDelete,
  onSave,
}: {
  storage: DebugStorageSnapshot | null;
  area: string;
  searchQuery: string;
  storageKey: string;
  storageValue: string;
  editingKey: string | null;
  saving: boolean;
  onKeyChange: (value: string) => void;
  onValueChange: (value: string) => void;
  onStartEdit: (area: string, key: string, value: string) => void;
  onStartAdd: (area: string) => void;
  onCancelEdit: () => void;
  onCopy: (value: string) => void;
  onDelete: (area: string, key: string) => void;
  onSave: () => void;
}) {
  const rowsViewportRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(0);
  const activeRows = useMemo(() => (storage ? storageRowsForArea(storage, area) : []), [area, storage]);
  const filteredRows = useMemo(
    () => filterBySearch(activeRows, searchQuery, ([key, value]) => `${key} ${value}`),
    [activeRows, searchQuery],
  );
  const virtualRange = useMemo(() => {
    const startIndex = Math.max(0, Math.floor(scrollTop / STORAGE_ROW_HEIGHT) - STORAGE_ROW_OVERSCAN);
    const visibleCount = Math.ceil((viewportHeight || 480) / STORAGE_ROW_HEIGHT) + STORAGE_ROW_OVERSCAN * 2;
    const endIndex = Math.min(filteredRows.length, startIndex + visibleCount);
    return { startIndex, endIndex };
  }, [filteredRows.length, scrollTop, viewportHeight]);
  const visibleRows = useMemo(
    () => filteredRows.slice(virtualRange.startIndex, virtualRange.endIndex),
    [filteredRows, virtualRange],
  );
  const handleRowsScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    setScrollTop(event.currentTarget.scrollTop);
  }, []);

  useLayoutEffect(() => {
    const element = rowsViewportRef.current;
    if (!element) return;
    const updateHeight = () => setViewportHeight(element.clientHeight);
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  if (!storage) return <Empty description="No storage snapshot yet" />;
  return (
    <div style={storageShellStyle}>
      <div style={storageToolbarStyle}>
        <Text type="secondary">{filteredRows.length} / {activeRows.length} item{activeRows.length === 1 ? "" : "s"}</Text>
        <Button
          size="small"
          icon={<PlusOutlined />}
          data-testid="devtools-storage-add"
          onClick={() => onStartAdd(area)}
        >
          Add
        </Button>
      </div>
      <div style={storageTableStyle}>
        <div style={storageHeaderRowStyle}>
          <Text strong>Key</Text>
          <Text strong>Value</Text>
          <Text strong>Actions</Text>
        </div>
        <div style={{ ...storageBodyStyle, gridTemplateRows: editingKey === "" ? "auto minmax(0, 1fr)" : "minmax(0, 1fr)" }}>
          {editingKey === "" ? (
            <StorageEditRow
              key="new"
              storageKey={storageKey}
              storageValue={storageValue}
              saving={saving}
              onKeyChange={onKeyChange}
              onValueChange={onValueChange}
              onSave={onSave}
              onCancel={onCancelEdit}
              onCopy={onCopy}
            />
          ) : null}
          {filteredRows.length ? (
            <div ref={rowsViewportRef} style={storageRowsViewportStyle} onScroll={handleRowsScroll}>
              <div style={{ ...storageRowsStyle, height: filteredRows.length * STORAGE_ROW_HEIGHT }}>
                {visibleRows.map(([key, value], offset) => {
                  const index = virtualRange.startIndex + offset;
                  return (
                    <div key={`${area}-${key}`} style={{ ...storageVirtualRowStyle, transform: `translateY(${index * STORAGE_ROW_HEIGHT}px)` }}>
                      {editingKey === key ? (
                        <StorageEditRow
                          storageKey={storageKey}
                          storageValue={storageValue}
                          saving={saving}
                          onKeyChange={onKeyChange}
                          onValueChange={onValueChange}
                          onSave={onSave}
                          onCancel={onCancelEdit}
                          onCopy={onCopy}
                        />
                      ) : (
                        <div data-testid="devtools-storage-row" style={storageRowStyle}>
                          <Text code ellipsis title={key}><HighlightedText text={key} query={searchQuery} /></Text>
                          <Text ellipsis title={value}><HighlightedText text={value} query={searchQuery} /></Text>
                          <Space size={4}>
                            <Button size="small" type="text" icon={<DeleteOutlined />} aria-label={`Delete ${key}`} onClick={() => onDelete(area, key)} />
                            <Button size="small" type="text" icon={<CopyOutlined />} aria-label={`Copy ${key}`} onClick={() => onCopy(value)} />
                            <Button size="small" type="text" icon={<EditOutlined />} aria-label={`Edit ${key}`} onClick={() => onStartEdit(area, key, value)} />
                          </Space>
                        </div>
                      )}
                      </div>
                  );
                })}
              </div>
            </div>
          ) : editingKey === "" ? null : (
            <div style={storageEmptyStyle}><Empty description="Empty" /></div>
          )}
        </div>
      </div>
    </div>
  );
}

function StorageEditRow({
  storageKey,
  storageValue,
  saving,
  onKeyChange,
  onValueChange,
  onSave,
  onCancel,
  onCopy,
}: {
  storageKey: string;
  storageValue: string;
  saving: boolean;
  onKeyChange: (value: string) => void;
  onValueChange: (value: string) => void;
  onSave: () => void;
  onCancel: () => void;
  onCopy: (value: string) => void;
}) {
  return (
    <div data-testid="devtools-storage-edit-row" style={storageRowStyle}>
      <Input
        data-testid="devtools-storage-key"
        value={storageKey}
        onChange={(event) => onKeyChange(event.target.value)}
        placeholder="Key"
      />
      <Input
        data-testid="devtools-storage-value"
        value={storageValue}
        onChange={(event) => onValueChange(event.target.value)}
        placeholder="Value"
        onPressEnter={onSave}
      />
      <Space size={4}>
        <Button size="small" icon={<CopyOutlined />} aria-label={`Copy ${storageKey}`} onClick={() => onCopy(storageValue)} />
        <Button size="small" type="primary" icon={<CheckOutlined />} loading={saving} data-testid="devtools-storage-save" aria-label="Save storage" onClick={onSave} />
        <Button size="small" icon={<CloseOutlined />} aria-label="Cancel storage edit" onClick={onCancel} />
      </Space>
    </div>
  );
}

function storageRowsForArea(storage: DebugStorageSnapshot, area: string): Array<[string, string]> {
  const rows =
    area === "cookie"
      ? storage.cookies
      : area === "session_storage"
        ? storage.session_storage.filter(([key]) => key !== "__bifrost_devtools_tab_id__")
        : storage.local_storage;
  return rows.slice().sort(([left], [right]) => left.localeCompare(right, undefined, { sensitivity: "base" }));
}

const storageShellStyle: CSSProperties = {
  display: "grid",
  gridTemplateRows: "auto minmax(0, 1fr)",
  gap: 0,
  height: "100%",
  minHeight: 0,
  border: "1px solid var(--devtools-border)",
  borderRadius: 6,
  overflow: "hidden",
  background: "var(--devtools-surface)",
  color: "var(--devtools-text)",
};

const storageToolbarStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 8,
  padding: "8px 10px",
  borderBottom: "1px solid var(--devtools-border)",
};

const storageTableStyle: CSSProperties = {
  display: "grid",
  gridTemplateRows: "auto minmax(0, 1fr)",
  minHeight: 0,
  overflow: "hidden",
  position: "relative",
};

const storageHeaderRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(180px, 32%) minmax(260px, 1fr) 132px",
  alignItems: "center",
  gap: 10,
  minWidth: 620,
  padding: "7px 10px",
  background: "var(--devtools-surface-alt)",
  borderBottom: "1px solid var(--devtools-border)",
};

const storageRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(180px, 32%) minmax(260px, 1fr) 132px",
  alignItems: "center",
  gap: 10,
  padding: "7px 10px",
  borderTop: "1px solid var(--devtools-border)",
  minWidth: 620,
  minHeight: 36,
};

const storageRowsStyle: CSSProperties = {
  minWidth: 620,
  position: "relative",
};

const storageBodyStyle: CSSProperties = {
  display: "grid",
  minHeight: 0,
};

const storageRowsViewportStyle: CSSProperties = {
  minHeight: 0,
  overflow: "auto",
  position: "relative",
};

const storageVirtualRowStyle: CSSProperties = {
  position: "absolute",
  top: 0,
  left: 0,
  width: "100%",
};

const storageEmptyStyle: CSSProperties = {
  padding: 18,
};
