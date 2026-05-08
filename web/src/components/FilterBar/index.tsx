import { Select, Input, Button, Space, AutoComplete, Tooltip } from "antd";
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
    };
    onFiltersChange([...filters, newFilter]);
  };

  const handleRemove = (id: string) => {
    onFiltersChange(filters.filter((f) => f.id !== id));
  };

  const handleChange = (
    id: string,
    key: keyof FilterCondition,
    value: string,
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
      />
    );
  };

  return (
    <div style={styles.container}>
      {filters.map((filter, index) => (
        <div key={filter.id} style={styles.row} data-testid="traffic-filter-row">
          <Select
            value={filter.field}
            options={fieldOptions}
            onChange={(value) => handleFieldChange(filter.id, value)}
            style={styles.fieldSelect}
            placeholder="Field"
            size="small"
          />
          <Select
            value={filter.operator}
            options={operatorOptions}
            onChange={(value) => handleChange(filter.id, "operator", value)}
            style={styles.operatorSelect}
            placeholder="Operator"
            size="small"
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
