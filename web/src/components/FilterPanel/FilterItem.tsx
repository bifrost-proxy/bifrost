import type { ReactNode, CSSProperties } from "react";
import { useMemo, useState, useCallback } from "react";
import { theme, Dropdown, Tooltip, message } from "antd";
import {
  CheckOutlined,
  EllipsisOutlined,
  PushpinOutlined,
  LockOutlined,
  UnlockOutlined,
} from "@ant-design/icons";
import type { MenuProps } from "antd";
import type { ItemType } from "antd/es/menu/interface";
import {
  useFilterPanelStore,
  isPinned,
  type FilterType,
} from "../../stores/useFilterPanelStore";
import { useTlsConfigStore } from "../../stores/useTlsConfigStore";
import { useI18n } from "../../i18n";

interface FilterItemProps {
  label: string;
  value: string;
  type: FilterType;
  selected: boolean;
  onSelect: () => void;
  onPin: () => void;
  icon?: ReactNode;
  count?: number;
  searchKeyword?: string;
}

function HighlightText({
  text,
  keyword,
  highlightColor,
}: {
  text: string;
  keyword?: string;
  highlightColor: string;
}) {
  if (!keyword || !keyword.trim()) {
    return <>{text}</>;
  }

  const lowerText = text.toLowerCase();
  const lowerKeyword = keyword.toLowerCase();
  const index = lowerText.indexOf(lowerKeyword);

  if (index === -1) {
    return <>{text}</>;
  }

  const before = text.slice(0, index);
  const match = text.slice(index, index + keyword.length);
  const after = text.slice(index + keyword.length);

  return (
    <>
      {before}
      <span style={{ backgroundColor: highlightColor, borderRadius: 2 }}>
        {match}
      </span>
      {after}
    </>
  );
}

export default function FilterItem({
  label,
  value,
  type,
  selected,
  onSelect,
  onPin,
  icon,
  count,
  searchKeyword,
}: FilterItemProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [isHovering, setIsHovering] = useState(false);
  const filterPanelState = useFilterPanelStore();
  const alreadyPinned = isPinned(filterPanelState, type, value);

  const {
    isAppInIntercept,
    isAppInPassthrough,
    isDomainInIntercept,
    isDomainInPassthrough,
    isIpInIntercept,
    isIpInPassthrough,
    addAppToIntercept,
    removeAppFromIntercept,
    addAppToPassthrough,
    removeAppFromPassthrough,
    addDomainToIntercept,
    removeDomainFromIntercept,
    addDomainToPassthrough,
    removeDomainFromPassthrough,
    addIpToIntercept,
    removeIpFromIntercept,
    addIpToPassthrough,
    removeIpFromPassthrough,
    config: tlsConfig,
  } = useTlsConfigStore();

  const isAppType = type === "client_app";
  const isDomainType = type === "domain";
  const isIpType = type === "client_ip";

  const inIntercept = isAppType
    ? isAppInIntercept(value)
    : isDomainType
      ? isDomainInIntercept(value)
      : isIpType
        ? isIpInIntercept(value)
        : false;

  const inPassthrough = isAppType
    ? isAppInPassthrough(value)
    : isDomainType
      ? isDomainInPassthrough(value)
      : isIpType
        ? isIpInPassthrough(value)
        : false;

  const handleEnableTlsIntercept = useCallback(async () => {
    let success = false;
    if (isAppType) {
      success = await addAppToIntercept(value);
      if (success) {
        message.success(t("traffic.filterItem.interceptEnabledDisconnected", { value }));
      }
    } else if (isDomainType) {
      success = await addDomainToIntercept(value);
      if (success) {
        message.success(t("traffic.filterItem.interceptEnabledDisconnected", { value }));
      }
    } else if (isIpType) {
      success = await addIpToIntercept(value);
      if (success) {
        message.success(t("traffic.filterItem.interceptEnabledIp", { value }));
      }
    }
    if (!success) {
      message.error(t("traffic.filterItem.interceptEnableFailed"));
    }
  }, [isAppType, isDomainType, isIpType, value, addAppToIntercept, addDomainToIntercept, addIpToIntercept, t]);

  const handleDisableTlsIntercept = useCallback(async () => {
    let success = false;
    if (isAppType) {
      success = await removeAppFromIntercept(value);
    } else if (isDomainType) {
      success = await removeDomainFromIntercept(value);
    } else if (isIpType) {
      success = await removeIpFromIntercept(value);
    }
    if (success) {
      message.success(t("traffic.filterItem.interceptRemoved", { value }));
    } else {
      message.error(t("traffic.filterItem.interceptDisableFailed"));
    }
  }, [
    t,
    isAppType,
    isDomainType,
    isIpType,
    value,
    removeAppFromIntercept,
    removeDomainFromIntercept,
    removeIpFromIntercept,
  ]);

  const handleEnablePassthrough = useCallback(async () => {
    let success = false;
    if (isAppType) {
      success = await addAppToPassthrough(value);
    } else if (isDomainType) {
      success = await addDomainToPassthrough(value);
    } else if (isIpType) {
      success = await addIpToPassthrough(value);
    }
    if (success) {
      message.success(t("traffic.filterItem.passthroughAdded", { value }));
    } else {
      message.error(t("traffic.filterItem.passthroughEnableFailed"));
    }
  }, [isAppType, isDomainType, isIpType, value, addAppToPassthrough, addDomainToPassthrough, addIpToPassthrough, t]);

  const handleDisablePassthrough = useCallback(async () => {
    let success = false;
    if (isAppType) {
      success = await removeAppFromPassthrough(value);
    } else if (isDomainType) {
      success = await removeDomainFromPassthrough(value);
    } else if (isIpType) {
      success = await removeIpFromPassthrough(value);
    }
    if (success) {
      message.success(t("traffic.filterItem.passthroughRemoved", { value }));
    } else {
      message.error(t("traffic.filterItem.passthroughDisableFailed"));
    }
  }, [
    t,
    isAppType,
    isDomainType,
    isIpType,
    value,
    removeAppFromPassthrough,
    removeDomainFromPassthrough,
    removeIpFromPassthrough,
  ]);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "5px 8px 5px 20px",
        cursor: "pointer",
        userSelect: "none",
        backgroundColor: selected ? token.colorPrimaryBg : "transparent",
        borderLeft: selected
          ? `2px solid ${token.colorPrimary}`
          : "2px solid transparent",
        transition: "all 0.15s",
      },
      containerHover: {
        backgroundColor: selected
          ? token.colorPrimaryBg
          : token.colorBgTextHover,
      },
      icon: {
        flexShrink: 0,
      },
      label: {
        flex: 1,
        fontSize: 12,
        color: selected ? token.colorPrimary : token.colorText,
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap" as const,
      },
      count: {
        fontSize: 11,
        color: selected ? token.colorPrimary : token.colorTextSecondary,
        flexShrink: 0,
        minWidth: 24,
        textAlign: "right" as const,
        fontVariantNumeric: "tabular-nums",
      },
      checkIcon: {
        fontSize: 12,
        color: token.colorPrimary,
        flexShrink: 0,
      },
      tlsIcon: {
        fontSize: 10,
        flexShrink: 0,
        marginLeft: -4,
      },
      moreBtn: {
        fontSize: 14,
        color: token.colorTextSecondary,
        padding: 2,
        borderRadius: 4,
        opacity: isHovering ? 1 : 0,
        transition: "opacity 0.15s",
        cursor: "pointer",
      },
      moreBtnHover: {
        backgroundColor: token.colorBgTextHover,
      },
    }),
    [token, selected, isHovering]
  );

  const menuItems: MenuProps["items"] = useMemo(() => {
    const items: ItemType[] = [
      {
        key: "pin",
        label: alreadyPinned ? t("traffic.filterItem.alreadyPinned") : t("traffic.filterItem.pinThisFilter"),
        icon: <PushpinOutlined />,
        disabled: alreadyPinned,
      },
    ];

    if ((isAppType || isDomainType || isIpType) && tlsConfig) {
      items.push({ type: "divider" });

      if (inIntercept) {
        items.push({
          key: "disable-intercept",
          label: t("traffic.filterItem.removeFromIntercept"),
          icon: <LockOutlined style={{ color: token.colorWarning }} />,
        });
      } else {
        items.push({
          key: "enable-intercept",
          label: t("traffic.filterItem.enableIntercept"),
          icon: <UnlockOutlined style={{ color: token.colorSuccess }} />,
        });
      }

      if (inPassthrough) {
        items.push({
          key: "disable-passthrough",
          label: t("traffic.filterItem.removeFromPassthrough"),
          icon: <UnlockOutlined style={{ color: token.colorSuccess }} />,
        });
      } else {
        items.push({
          key: "enable-passthrough",
          label: t("traffic.filterItem.addToPassthrough"),
          icon: <LockOutlined style={{ color: token.colorWarning }} />,
        });
      }
    }

    return items;
  }, [
    t,
    alreadyPinned,
    isAppType,
    isDomainType,
    isIpType,
    tlsConfig,
    inIntercept,
    inPassthrough,
    token,
  ]);

  const handleMenuClick: MenuProps["onClick"] = useCallback(
    ({ key }: { key: string }) => {
      switch (key) {
        case "pin":
          if (!alreadyPinned) {
            onPin();
          }
          break;
        case "enable-intercept":
          handleEnableTlsIntercept();
          break;
        case "disable-intercept":
          handleDisableTlsIntercept();
          break;
        case "enable-passthrough":
          handleEnablePassthrough();
          break;
        case "disable-passthrough":
          handleDisablePassthrough();
          break;
      }
    },
    [
      alreadyPinned,
      onPin,
      handleEnableTlsIntercept,
      handleDisableTlsIntercept,
      handleEnablePassthrough,
      handleDisablePassthrough,
    ]
  );

  const tlsIndicator = useMemo(() => {
    if (inIntercept) {
      return (
        <Tooltip title={t("traffic.filterItem.interceptIndicator")}>
          <UnlockOutlined
            style={{ ...styles.tlsIcon, color: token.colorSuccess }}
          />
        </Tooltip>
      );
    }
    if (inPassthrough) {
      return (
        <Tooltip title={t("traffic.filterItem.passthroughIndicator")}>
          <LockOutlined
            style={{ ...styles.tlsIcon, color: token.colorWarning }}
          />
        </Tooltip>
      );
    }
    return null;
  }, [inIntercept, inPassthrough, styles.tlsIcon, token, t]);

  return (
    <div
      data-testid={`filter-item-${type}`}
      data-filter-value={value}
      style={{
        ...styles.container,
        ...(isHovering ? styles.containerHover : {}),
      }}
      onMouseEnter={() => setIsHovering(true)}
      onMouseLeave={() => setIsHovering(false)}
      onClick={onSelect}
    >
      {icon && <span style={styles.icon}>{icon}</span>}
      <Tooltip title={label} placement="right" mouseEnterDelay={0.5}>
        <span style={styles.label}>
          <HighlightText
            text={label}
            keyword={searchKeyword}
            highlightColor={token.colorWarningBg}
          />
        </span>
      </Tooltip>
      {tlsIndicator}
      {typeof count === "number" && (
        <span
          style={styles.count}
          data-testid={`filter-item-count-${type}`}
          aria-label={t("traffic.filterPanel.countAria", { label })}
        >
          {count.toLocaleString()}
        </span>
      )}
      {selected && <CheckOutlined style={styles.checkIcon} />}
      <Dropdown
        menu={{ items: menuItems, onClick: handleMenuClick }}
        trigger={["click"]}
        placement="bottomRight"
      >
        <span
          style={styles.moreBtn}
          onClick={(e) => e.stopPropagation()}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor = token.colorBgTextHover;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
        >
          <EllipsisOutlined />
        </span>
      </Dropdown>
    </div>
  );
}
