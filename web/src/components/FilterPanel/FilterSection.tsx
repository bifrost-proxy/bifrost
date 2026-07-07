import type { ReactNode, CSSProperties } from "react";
import { useMemo } from "react";
import { theme, Typography, Badge } from "antd";
import { CaretRightOutlined, CaretDownOutlined } from "@ant-design/icons";

const { Text } = Typography;

interface FilterSectionProps {
  title: string;
  icon?: string;
  collapsed: boolean;
  onToggle: () => void;
  count?: number;
  children: ReactNode;
}

export default function FilterSection({
  title,
  icon,
  collapsed,
  onToggle,
  count,
  children,
}: FilterSectionProps) {
  const { token } = theme.useToken();
  const testId = `filter-section-${title.toLowerCase().replace(/\s+/g, "-")}`;

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        borderBottom: `1px solid ${token.colorBorderSecondary}`,
      },
      header: {
        display: "flex",
        alignItems: "center",
        gap: 4,
        padding: "6px 8px",
        cursor: "pointer",
        userSelect: "none",
        backgroundColor: token.colorBgLayout,
      },
      headerHover: {
        backgroundColor: token.colorBgTextHover,
      },
      icon: {
        fontSize: 10,
        color: token.colorTextSecondary,
        transition: "transform 0.2s",
      },
      titleIcon: {
        fontSize: 12,
      },
      title: {
        fontSize: 12,
        fontWeight: 500,
        color: token.colorText,
        flex: 1,
      },
      count: {
        fontSize: 11,
        color: token.colorTextSecondary,
      },
      content: {
        display: collapsed ? "none" : "block",
      },
    }),
    [token, collapsed]
  );

  return (
    <div style={styles.container} data-testid={testId}>
      <div
        style={styles.header}
        onClick={onToggle}
        onMouseEnter={(e) => {
          e.currentTarget.style.backgroundColor = token.colorBgTextHover;
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.backgroundColor = token.colorBgLayout;
        }}
      >
        {collapsed ? (
          <CaretRightOutlined style={styles.icon} />
        ) : (
          <CaretDownOutlined style={styles.icon} />
        )}
        {icon && <span style={styles.titleIcon}>{icon}</span>}
        <Text style={styles.title}>{title}</Text>
        {count !== undefined && count > 0 && (
          <Badge
            count={count}
            size="small"
            style={{
              backgroundColor: token.colorBgTextHover,
              color: token.colorTextSecondary,
              fontSize: 10,
            }}
          />
        )}
      </div>
      <div style={styles.content}>{children}</div>
    </div>
  );
}
