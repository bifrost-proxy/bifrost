import { useMemo, useState, useCallback, useRef, useEffect } from "react";
import { Input, Button, Dropdown, Modal, message, Tooltip, Spin, Select } from "antd";
import type { MenuProps } from "antd";
import {
  PlusOutlined,
  ReloadOutlined,
  SearchOutlined,
  EditOutlined,
  DeleteOutlined,
  CopyOutlined,
  ExportOutlined,
  MoreOutlined,
} from "@ant-design/icons";
import { useValuesStore } from "../../../stores/useValuesStore";
import { ImportBifrostButton } from "../../../components/ImportBifrostButton";
import { useExportBifrost } from "../../../hooks/useExportBifrost";
import { copyToClipboard } from "../../../utils/clipboard";
import { useI18n, type TFunction } from "../../../i18n";
import styles from "./index.module.css";

type ValueSortMode = "created_desc" | "updated_desc" | "name_asc";

const getValueSortOptions = (t: TFunction) => [
  { label: t("values.list.sortNewest"), value: "created_desc" },
  { label: t("values.list.sortUpdated"), value: "updated_desc" },
  { label: t("values.list.sortName"), value: "name_asc" },
];

function getValueItemId(name: string) {
  return `value-item-${encodeURIComponent(name)}`;
}

export default function ValueList() {
  const { t } = useI18n();
  const valueSortOptions = useMemo(() => getValueSortOptions(t), [t]);
  const {
    values,
    selectedValueName,
    searchKeyword,
    loading,
    editingContent,
    fetchValues,
    selectValue,
    createValue,
    deleteValue,
    renameValue,
    setSearchKeyword,
    hasUnsavedChanges,
  } = useValuesStore();

  const [createModalVisible, setCreateModalVisible] = useState(false);
  const [newValueName, setNewValueName] = useState("");
  const [renameModalVisible, setRenameModalVisible] = useState(false);
  const [renameTarget, setRenameTarget] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [selectedValues, setSelectedValues] = useState<string[]>([]);
  const lastClickedIndexRef = useRef<number | null>(null);
  const [sortMode, setSortMode] = useState<ValueSortMode>("created_desc");
  const listContainerRef = useRef<HTMLDivElement | null>(null);
  const { exportFile } = useExportBifrost();

  const filteredValues = useMemo(() => {
    const sortedValues = [...values].sort((left, right) => {
      if (sortMode === "updated_desc") {
        return (
          Date.parse(right.updated_at) - Date.parse(left.updated_at) ||
          left.name.localeCompare(right.name)
        );
      }
      if (sortMode === "name_asc") {
        return left.name.localeCompare(right.name);
      }
      return (
        Date.parse(right.created_at) - Date.parse(left.created_at) ||
        left.name.localeCompare(right.name)
      );
    });
    if (!searchKeyword) return sortedValues;
    const keyword = searchKeyword.toLowerCase();
    return sortedValues.filter(
      (v) =>
        v.name.toLowerCase().includes(keyword) ||
        v.value.toLowerCase().includes(keyword),
    );
  }, [values, searchKeyword, sortMode]);

  const handleCreate = async () => {
    if (!newValueName.trim()) {
      message.error(t("values.list.nameRequired"));
      return;
    }
    const success = await createValue(newValueName.trim(), "");
    if (success) {
      message.success(t("values.list.created"));
      setCreateModalVisible(false);
      setNewValueName("");
    }
  };

  const handleDelete = async (name: string) => {
    Modal.confirm({
      title: t("values.deleteTitle"),
      content: t("values.deleteContent", { name }),
      okText: t("common.delete"),
      okType: "danger",
      cancelText: t("common.cancel"),
      onOk: async () => {
        const success = await deleteValue(name);
        if (success) {
          message.success(t("values.deleted"));
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
      title: t("values.list.bulkDeleteTitle"),
      content: t("values.list.bulkDeleteContent", { count: names.length }),
      okText: t("common.delete"),
      okType: "danger",
      cancelText: t("common.cancel"),
      onOk: async () => {
        let successCount = 0;
        for (const name of names) {
          const success = await deleteValue(name);
          if (success) successCount++;
        }
        if (successCount > 0) {
          message.success(
            t(
              successCount > 1
                ? "values.list.bulkDeletedOther"
                : "values.list.bulkDeletedOne",
              { count: successCount },
            ),
          );
          setSelectedValues([]);
        }
      },
    });
  };

  const handleRename = async () => {
    if (!renameTarget || !newName.trim()) return;
    if (newName.trim() === renameTarget) {
      setRenameModalVisible(false);
      return;
    }
    const success = await renameValue(renameTarget, newName.trim());
    if (success) {
      message.success(t("values.list.renamed"));
      setRenameModalVisible(false);
      setRenameTarget(null);
      setNewName("");
    }
  };

  const handleCopy = async (name: string, value: string) => {
    try {
      await copyToClipboard(value);
      message.success(t("values.list.copiedToClipboard", { name }));
    } catch {
      message.error(t("values.copyFailed"));
    }
  };

  const handleExport = useCallback(
    async (names: string[]) => {
      if (names.length === 0) return;
      await exportFile("values", { value_names: names });
    },
    [exportFile],
  );

  const handleExportAll = useCallback(async () => {
    await exportFile("values", {});
  }, [exportFile]);

  const handleImportSuccess = useCallback(() => {
    fetchValues();
  }, [fetchValues]);

  const handleSelect = useCallback(
    (name: string, e: React.MouseEvent) => {
      const isCtrl = e.ctrlKey || e.metaKey;
      const isShift = e.shiftKey;
      const currentIndex = filteredValues.findIndex((v) => v.name === name);

      if (isShift && lastClickedIndexRef.current !== null) {
        const start = Math.min(lastClickedIndexRef.current, currentIndex);
        const end = Math.max(lastClickedIndexRef.current, currentIndex);
        const rangeNames = filteredValues
          .slice(start, end + 1)
          .map((v) => v.name);
        setSelectedValues((prev) => {
          const combined = new Set([...prev, ...rangeNames]);
          return Array.from(combined);
        });
      } else if (isCtrl) {
        setSelectedValues((prev) =>
          prev.includes(name)
            ? prev.filter((n) => n !== name)
            : [...prev, name],
        );
        lastClickedIndexRef.current = currentIndex;
      } else {
        setSelectedValues([]);
        lastClickedIndexRef.current = currentIndex;
        selectValue(name);
      }
    },
    [selectValue, filteredValues],
  );

  const getContextMenuItems = (
    name: string,
    value: string,
  ): MenuProps["items"] => {
    const isSelected = selectedValues.includes(name);
    const bulkNames =
      isSelected && selectedValues.length > 0 ? selectedValues : [name];

    return [
      {
        key: "copy",
        icon: <CopyOutlined />,
        label: t("values.list.menuCopyValue"),
        onClick: () => handleCopy(name, value),
      },
      {
        key: "rename",
        icon: <EditOutlined />,
        label: t("common.rename"),
        onClick: () => {
          setRenameTarget(name);
          setNewName(name);
          setRenameModalVisible(true);
        },
      },
      {
        type: "divider",
      },
      {
        key: "export",
        icon: <ExportOutlined />,
        label:
          bulkNames.length > 1
            ? t("values.list.menuExportCount", { count: bulkNames.length })
            : t("common.export"),
        onClick: () => handleExport(bulkNames),
      },
      {
        type: "divider",
      },
      {
        key: "delete",
        icon: <DeleteOutlined />,
        label:
          bulkNames.length > 1
            ? t("values.list.menuDeleteCount", { count: bulkNames.length })
            : t("common.delete"),
        danger: true,
        onClick: () => handleBulkDelete(bulkNames),
      },
    ];
  };

  const handleListKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) {
        return;
      }

      if (filteredValues.length === 0) {
        return;
      }

      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
        return;
      }

      event.preventDefault();

      const currentIndex = selectedValueName
        ? filteredValues.findIndex((value) => value.name === selectedValueName)
        : -1;

      const fallbackIndex =
        event.key === "ArrowDown" ? 0 : filteredValues.length - 1;
      const nextIndex =
        currentIndex === -1
          ? fallbackIndex
          : Math.min(
              filteredValues.length - 1,
              Math.max(0, currentIndex + (event.key === "ArrowDown" ? 1 : -1)),
            );

      const nextValue = filteredValues[nextIndex];
      if (!nextValue || nextValue.name === selectedValueName) {
        return;
      }

      setSelectedValues([]);
      lastClickedIndexRef.current = nextIndex;
      selectValue(nextValue.name);
    },
    [filteredValues, selectedValueName, selectValue],
  );

  useEffect(() => {
    if (!selectedValueName) {
      return;
    }

    const selectedItem = listContainerRef.current?.querySelector<HTMLElement>(
      `[data-value-name="${CSS.escape(selectedValueName)}"]`,
    );
    selectedItem?.scrollIntoView({ block: "nearest" });
  }, [filteredValues, selectedValueName]);

  return (
    <div className={styles.container} data-testid="values-list">
      <div className={styles.header}>
        <span className={styles.headerTitle}>{t("values.list.title")}</span>
        <div className={styles.headerActions}>
          <Tooltip title={t("values.list.newValue")}>
            <Button
              type="text"
              size="small"
              icon={<PlusOutlined />}
              onClick={() => setCreateModalVisible(true)}
              data-testid="value-new-button"
            />
          </Tooltip>
          <Tooltip title={t("common.refresh")}>
            <Button
              type="text"
              size="small"
              icon={<ReloadOutlined />}
              onClick={() => fetchValues()}
              data-testid="value-refresh-button"
            />
          </Tooltip>
          <ImportBifrostButton
            expectedType="values"
            onImportSuccess={handleImportSuccess}
            buttonText=""
            buttonType="text"
            size="small"
            testId="value-import-button"
          />
          <Tooltip title={t("values.list.exportAll")}>
            <Button
              type="text"
              size="small"
              icon={<ExportOutlined />}
              onClick={handleExportAll}
              data-testid="value-export-all-button"
            />
          </Tooltip>
        </div>
      </div>
      <div className={styles.searchBox}>
        <Input
          size="small"
          placeholder={t("values.list.searchPlaceholder")}
          prefix={<SearchOutlined style={{ color: "#999" }} />}
          value={searchKeyword}
          onChange={(e) => setSearchKeyword(e.target.value)}
          allowClear
          data-testid="value-search-input"
        />
        <Select
          size="small"
          value={sortMode}
          onChange={(value: ValueSortMode) => setSortMode(value)}
          options={valueSortOptions}
          className={styles.sortControl}
          popupMatchSelectWidth={false}
          data-testid="value-sort-select"
        />
      </div>

      <div
        ref={listContainerRef}
        className={styles.listContainer}
        tabIndex={0}
        role="listbox"
        aria-label={t("values.list.ariaLabel")}
        aria-activedescendant={
          selectedValueName ? getValueItemId(selectedValueName) : undefined
        }
        onKeyDown={handleListKeyDown}
      >
        {loading && values.length === 0 ? (
          <div className={styles.loading}>
            <Spin size="small" />
          </div>
        ) : (
          <div className={styles.list}>
            {filteredValues.map((item) => {
              const isSelected = selectedValueName === item.name;
              const hasChanges =
                hasUnsavedChanges(item.name) ||
                editingContent[item.name] !== undefined;

              return (
                <Dropdown
                  key={item.name}
                  menu={{
                    items: getContextMenuItems(item.name, item.value),
                  }}
                  trigger={["contextMenu"]}
                >
                  <div
                    id={getValueItemId(item.name)}
                    className={`${styles.item} ${isSelected ? styles.selected : ""} ${selectedValues.includes(item.name) ? styles.multiSelected : ""}`}
                    role="option"
                    aria-selected={isSelected}
                    onClick={(e) => {
                      listContainerRef.current?.focus();
                      handleSelect(item.name, e);
                    }}
                    data-testid="value-item"
                    data-value-name={item.name}
                  >
                    <div className={styles.itemContent}>
                      <span className={styles.itemName} title={item.name}>
                        {item.name}
                      </span>
                      <div className={styles.itemMeta}>
                        {hasChanges && (
                          <Tooltip title={t("values.list.unsavedChanges")}>
                            <span className={styles.unsavedDot} />
                          </Tooltip>
                        )}
                      </div>
                    </div>
                    <div className={styles.itemExtra}>
                      <Dropdown
                        menu={{
                          items: getContextMenuItems(item.name, item.value),
                        }}
                        trigger={["click"]}
                      >
                        <Button
                          type="text"
                          size="small"
                          icon={<MoreOutlined />}
                          onClick={(e) => e.stopPropagation()}
                          className={styles.moreBtn}
                          data-testid="value-item-menu"
                        />
                      </Dropdown>
                    </div>
                  </div>
                </Dropdown>
              );
            })}
            {filteredValues.length === 0 && !loading && (
              <div className={styles.empty}>
                {searchKeyword
                  ? t("values.list.emptyNoMatch")
                  : t("values.list.emptyNoValues")}
              </div>
            )}
          </div>
        )}
      </div>

      <div className={styles.footer}>
        <span className={styles.stats}>
          {t("values.list.stats", { count: values.length })}
        </span>
      </div>

      <Modal
        title={t("values.list.newValue")}
        open={createModalVisible}
        onCancel={() => {
          setCreateModalVisible(false);
          setNewValueName("");
        }}
        onOk={handleCreate}
        okText={t("common.create")}
        cancelText={t("common.cancel")}
      >
        <Input
          placeholder={t("values.list.namePlaceholder")}
          value={newValueName}
          onChange={(e) => setNewValueName(e.target.value)}
          onPressEnter={handleCreate}
          autoFocus
        />
      </Modal>

      <Modal
        title={t("values.list.renameTitle")}
        open={renameModalVisible}
        onCancel={() => {
          setRenameModalVisible(false);
          setRenameTarget(null);
          setNewName("");
        }}
        onOk={handleRename}
        okText={t("common.rename")}
        cancelText={t("common.cancel")}
      >
        <Input
          placeholder={t("values.list.newNamePlaceholder")}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onPressEnter={handleRename}
          autoFocus
        />
      </Modal>
    </div>
  );
}
