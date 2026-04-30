import { useMemo, useRef, type CSSProperties } from "react";
import { CheckOutlined, CloseOutlined, CopyOutlined, DeleteOutlined, EditOutlined, PlusOutlined } from "@ant-design/icons";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Button, Empty, Input, Space, Typography } from "antd";
import type { DebugStorageSnapshot } from "../../../api/devtools";
import { HighlightedText, filterBySearch } from "./shared";

const { Text } = Typography;

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
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const activeRows = useMemo(() => (storage ? storageRowsForArea(storage, area) : []), [area, storage]);
  const filteredRows = useMemo(
    () => filterBySearch(activeRows, searchQuery, ([key, value]) => `${key} ${value}`),
    [activeRows, searchQuery],
  );
  const rowVirtualizer = useVirtualizer({
    count: filteredRows.length,
    getScrollElement: () => scrollerRef.current,
    estimateSize: () => 36,
    overscan: 12,
  });
  const virtualRows = rowVirtualizer.getVirtualItems();
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
      <div ref={scrollerRef} style={storageTableStyle}>
        <div style={storageHeaderRowStyle}>
          <Text strong>Key</Text>
          <Text strong>Value</Text>
          <Text strong>Actions</Text>
        </div>
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
          <div style={{ ...storageVirtualSpaceStyle, height: rowVirtualizer.getTotalSize() }}>
            {virtualRows.map((virtualRow) => {
              const [key, value] = filteredRows[virtualRow.index];
              return (
                <div
                  key={`${area}-${key}`}
                  data-index={virtualRow.index}
                  ref={rowVirtualizer.measureElement}
                  style={{
                    ...storageVirtualRowStyle,
                    transform: `translateY(${virtualRow.start}px)`,
                  }}
                >
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
        ) : editingKey === "" ? null : (
          <div style={storageEmptyStyle}><Empty description="Empty" /></div>
        )}
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
  if (area === "cookie") return storage.cookies;
  if (area === "session_storage") return storage.session_storage;
  return storage.local_storage;
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
  minHeight: 0,
  overflow: "auto",
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

const storageVirtualSpaceStyle: CSSProperties = {
  position: "relative",
  minWidth: 620,
};

const storageVirtualRowStyle: CSSProperties = {
  position: "absolute",
  top: 0,
  left: 0,
  right: 0,
  minWidth: 620,
};

const storageEmptyStyle: CSSProperties = {
  padding: 18,
};
