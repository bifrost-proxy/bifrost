import {
  Select,
  Input,
  Button,
  Space,
  AutoComplete,
  Tooltip,
  Checkbox,
} from "antd";
import { PlusOutlined, DeleteOutlined, SearchOutlined } from "@ant-design/icons";
import { useMemo } from "react";
import type { FilterCondition } from "../../types";

interface FilterBarProps {
  filters: FilterCondition[];
  onFiltersChange: (filters: FilterCondition[]) => void;
  availableClientApps?: string[];
  availableClientIps?: string[];
  onSearchModeToggle?: () => void;
  isSearchMode?: boolean;
}

const fieldOptions = [
  { value: "url", label: "URL" },
  { value: "host", label: "Host" },
  { value: "path", label: "Path" },
  { value: "method", label: "Method" },
  { value: "client_app", label: "Client App" },
  { value: "client_ip", label: "Client IP" },
  { value: "listener_port", label: "Port" },
  { value: "content_type", label: "Content-Type" },
];

const operatorOptions = [
  { value: "contains", label: "Contains" },
  { value: "equals", label: "Equals" },
  { value: "regex", label: "Regex" },
  { value: "not_contains", label: "Not Contains" },
  { value: "is_empty", label: "Is Empty" },
  { value: "is_not_empty", label: "Is Not Empty" },
];

const styles = {
  container: {
    display: "flex",
    flexDirection: "column" as const,
    gap: 8,
  },
  row: {
    display: "flex",
    alignItems: "center",
    gap: 8,
  },
  disabledRow: {
    opacity: 0.62,
  },
  enabledCheckbox: {
    flex: "0 0 auto",
  },
  fieldSelect: {
    width: 140,
  },
  operatorSelect: {
    width: 110,
  },
  valueInput: {
    flex: 1,
    minWidth: 150,
  },
};

export default function FilterBar({
  filters,
  onFiltersChange,
  availableClientApps = [],
  availableClientIps = [],
  onSearchModeToggle,
  isSearchMode = false,
}: FilterBarProps) {
  const generateId = () =>
    `filter_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;

  const handleAdd = () => {
    const newFilter: FilterCondition = {
      id: generateId(),
      field: "url",
      operator: "contains",
      value: "",
      enabled: true,
    };
    onFiltersChange([...filters, newFilter]);
  };

  const handleRemove = (id: string) => {
    onFiltersChange(filters.filter((f) => f.id !== id));
  };

  const handleChange = <K extends keyof FilterCondition>(
    id: string,
    key: K,
    value: FilterCondition[K],
  ) => {
    onFiltersChange(
      filters.map((f) => (f.id === id ? { ...f, [key]: value } : f)),
    );
  };

  const handleFieldChange = (id: string, field: string) => {
    onFiltersChange(
      filters.map((f) =>
        f.id === id
          ? {
              ...f,
              field,
              operator: field === "listener_port" ? "equals" : f.operator,
              value: "",
            }
          : f,
      ),
    );
  };

  const clientAppOptions = useMemo(() => {
    return availableClientApps.map((app) => ({
      value: app,
      label: app,
    }));
  }, [availableClientApps]);

  const clientIpOptions = useMemo(() => {
    return availableClientIps.map((ip) => ({
      value: ip,
      label: ip,
    }));
  }, [availableClientIps]);

  const renderValueInput = (filter: FilterCondition) => {
    const disabled = filter.enabled === false;

    if (filter.operator === "is_empty" || filter.operator === "is_not_empty") {
      return (
        <Input
          value=""
          disabled
          style={styles.valueInput}
          placeholder="No value needed"
          size="small"
        />
      );
    }

    if (filter.field === "client_app") {
      return (
        <AutoComplete
          value={filter.value}
          options={clientAppOptions}
          onChange={(value) => handleChange(filter.id, "value", value)}
          style={styles.valueInput}
          placeholder="Select or enter app name..."
          size="small"
          disabled={disabled}
          filterOption={(inputValue, option) =>
            option?.value.toLowerCase().includes(inputValue.toLowerCase()) ?? false
          }
          allowClear
        />
      );
    }

    if (filter.field === "client_ip") {
      return (
        <AutoComplete
          value={filter.value}
          options={clientIpOptions}
          onChange={(value) => handleChange(filter.id, "value", value)}
          style={styles.valueInput}
          placeholder="Select or enter IP address..."
          size="small"
          disabled={disabled}
          filterOption={(inputValue, option) =>
            option?.value.toLowerCase().includes(inputValue.toLowerCase()) ?? false
          }
          allowClear
        />
      );
    }

    if (filter.field === "listener_port") {
      return (
        <Input
          value={filter.value}
          onChange={(e) => handleChange(filter.id, "value", e.target.value)}
          style={styles.valueInput}
          placeholder="Enter proxy port..."
          size="small"
          disabled={disabled}
          inputMode="numeric"
        />
      );
    }

    return (
      <Input
        value={filter.value}
        onChange={(e) => handleChange(filter.id, "value", e.target.value)}
        style={styles.valueInput}
        placeholder="Enter value..."
        size="small"
        disabled={disabled}
      />
    );
  };

  return (
    <div style={styles.container}>
      {filters.map((filter, index) => (
        <div
          key={filter.id}
          style={{
            ...styles.row,
            ...(filter.enabled === false ? styles.disabledRow : {}),
          }}
          data-testid="traffic-filter-row"
        >
          <Tooltip
            title={
              filter.enabled === false
                ? "Enable this filter"
                : "Temporarily disable this filter"
            }
          >
            <Checkbox
              checked={filter.enabled !== false}
              onChange={(e) =>
                handleChange(filter.id, "enabled", e.target.checked)
              }
              aria-label="Enable filter"
              data-testid="traffic-filter-enabled-checkbox"
              style={styles.enabledCheckbox}
            />
          </Tooltip>
          <Select
            value={filter.field}
            options={fieldOptions}
            onChange={(value) => handleFieldChange(filter.id, value)}
            style={styles.fieldSelect}
            placeholder="Field"
            size="small"
            disabled={filter.enabled === false}
          />
          <Select
            value={filter.operator}
            options={operatorOptions}
            onChange={(value) => handleChange(filter.id, "operator", value)}
            style={styles.operatorSelect}
            placeholder="Operator"
            size="small"
            disabled={filter.enabled === false}
          />
          {renderValueInput(filter)}
          <Space size={4}>
            <Button
              type="text"
              size="small"
              danger
              icon={<DeleteOutlined />}
              onClick={() => handleRemove(filter.id)}
            />
            {index === filters.length - 1 && (
              <Button
                type="primary"
                size="small"
                icon={<PlusOutlined />}
                onClick={handleAdd}
              />
            )}
          </Space>
        </div>
      ))}
      <div style={{ ...styles.row, justifyContent: "space-between" }}>
        {filters.length === 0 ? (
          <Button
            type="dashed"
            size="small"
            icon={<PlusOutlined />}
            onClick={handleAdd}
            style={{ flex: 1 }}
          >
            Add Filter
          </Button>
        ) : (
          <div />
        )}
        {onSearchModeToggle && (
          <Tooltip title="Search in body, headers, and URL content">
            <Button
              type={isSearchMode ? "primary" : "default"}
              size="small"
              icon={<SearchOutlined />}
              onClick={onSearchModeToggle}
            >
              Fuzzy Search
            </Button>
          </Tooltip>
        )}
      </div>
    </div>
  );
}
