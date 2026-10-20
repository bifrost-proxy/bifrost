import { useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Divider,
  Input,
  Popconfirm,
  Row,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
  Tooltip,
  message,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  ClockCircleOutlined,
  ClearOutlined,
  DeleteOutlined,
  GlobalOutlined,
  LaptopOutlined,
  PlusOutlined,
  ReloadOutlined,
  SafetyOutlined,
  StopOutlined,
} from "@ant-design/icons";
import { useWhitelistStore } from "../../../stores/useWhitelistStore";
import type { AccessMode, UserPassStatus } from "../../../types";
import { notifyApiBusinessError } from "../../../api/client";
import { useI18n, type TFunction } from "../../../i18n";

const { Text } = Typography;

const getAccessModeOptions = (
  t: TFunction,
): {
  value: AccessMode;
  label: string;
  description: string;
}[] => [
  {
    value: "allow_all",
    label: t("settings.access.modeAllowAll"),
    description: t("settings.access.modeAllowAllDescription"),
  },
  {
    value: "local_only",
    label: t("settings.access.modeLocalOnly"),
    description: t("settings.access.modeLocalOnlyDescription"),
  },
  {
    value: "whitelist",
    label: t("settings.access.modeWhitelist"),
    description: t("settings.access.modeWhitelistDescription"),
  },
  {
    value: "interactive",
    label: t("settings.access.modeInteractive"),
    description: t("settings.access.modeInteractiveDescription"),
  },
];

const getModeColor = (mode: AccessMode) => {
  switch (mode) {
    case "allow_all":
      return "red";
    case "local_only":
      return "blue";
    case "whitelist":
      return "green";
    case "interactive":
      return "orange";
    default:
      return "default";
  }
};

const getModeIcon = (mode: AccessMode) => {
  switch (mode) {
    case "allow_all":
      return <GlobalOutlined />;
    case "local_only":
      return <LaptopOutlined />;
    case "whitelist":
      return <SafetyOutlined />;
    case "interactive":
      return <ClockCircleOutlined />;
    default:
      return null;
  }
};

interface UserPassAccountDraft {
  key: string;
  username: string;
  password: string;
  enabled: boolean;
  hasPassword: boolean;
  lastConnectedAt: string | null;
}

interface UserPassDraftState {
  enabled: boolean;
  loopbackRequiresAuth: boolean;
  accounts: UserPassAccountDraft[];
}

export const createUserPassDraft = (
  userpass: UserPassStatus | null | undefined,
): UserPassDraftState => ({
  enabled: userpass?.enabled ?? false,
  loopbackRequiresAuth: userpass?.loopback_requires_auth ?? false,
  accounts:
    userpass?.accounts.map((account) => ({
      key: account.username,
      username: account.username,
      password: "",
      enabled: account.enabled,
      hasPassword: account.has_password,
      lastConnectedAt: account.last_connected_at,
    })) ?? [],
});

export const userPassConfigSignature = (
  userpass: UserPassStatus | null | undefined,
): string | null => {
  if (!userpass) {
    return null;
  }

  return JSON.stringify({
    enabled: userpass.enabled,
    loopbackRequiresAuth: userpass.loopback_requires_auth ?? false,
    accounts: userpass.accounts.map((account) => ({
      username: account.username,
      enabled: account.enabled,
      hasPassword: account.has_password,
    })),
  });
};

export const mergeUserPassRuntimeState = (
  accounts: UserPassAccountDraft[],
  userpass: UserPassStatus | null | undefined,
): UserPassAccountDraft[] => {
  if (!userpass) {
    return accounts;
  }

  const lastConnectedAtByUsername = new Map(
    userpass.accounts.map((account) => [account.username, account.last_connected_at]),
  );
  let changed = false;
  const merged = accounts.map((account) => {
    const lastConnectedAt = lastConnectedAtByUsername.get(account.username);
    if (lastConnectedAt === undefined || lastConnectedAt === account.lastConnectedAt) {
      return account;
    }
    changed = true;
    return { ...account, lastConnectedAt };
  });

  return changed ? merged : accounts;
};

export default function AccessControlTab() {
  const { t } = useI18n();
  const accessModeOptions = useMemo(() => getAccessModeOptions(t), [t]);
  const {
    status,
    loading,
    error,
    fetchStatus,
    addToWhitelist,
    removeFromWhitelist,
    setMode,
    setAllowLan,
    setUserPassConfig,
    addTemporary,
    removeTemporary,
    removeSessionDenied,
    clearSessionDenied,
    clearError,
  } = useWhitelistStore();

  const [newIpOrCidr, setNewIpOrCidr] = useState("");
  const [newTempIp, setNewTempIp] = useState("");

  useEffect(() => {
    if (error) {
      notifyApiBusinessError(new Error(error), error);
      clearError();
    }
  }, [error, clearError]);

  const derivedUserPass = useMemo(
    () => createUserPassDraft(status?.userpass),
    [status?.userpass],
  );
  const userPassSignature = useMemo(
    () => userPassConfigSignature(status?.userpass),
    [status?.userpass],
  );

  const [userPassEnabled, setUserPassEnabled] = useState(
    () => derivedUserPass.enabled,
  );
  const [loopbackRequiresAuth, setLoopbackRequiresAuth] = useState(
    () => derivedUserPass.loopbackRequiresAuth,
  );
  const [userPassAccounts, setUserPassAccounts] = useState<UserPassAccountDraft[]>(
    () => derivedUserPass.accounts,
  );
  const lastSyncedUserPassSignature = useRef(userPassSignature);

  useEffect(() => {
    if (
      userPassSignature === null ||
      userPassSignature === lastSyncedUserPassSignature.current
    ) {
      return;
    }

    lastSyncedUserPassSignature.current = userPassSignature;
    setUserPassEnabled(derivedUserPass.enabled);
    setLoopbackRequiresAuth(derivedUserPass.loopbackRequiresAuth);
    setUserPassAccounts(derivedUserPass.accounts);
  }, [derivedUserPass, userPassSignature]);

  useEffect(() => {
    setUserPassAccounts((current) =>
      mergeUserPassRuntimeState(current, status?.userpass),
    );
  }, [status?.userpass]);

  const handleAdd = async () => {
    if (!newIpOrCidr.trim()) {
      message.warning(t("settings.access.enterIpOrCidr"));
      return;
    }
    const success = await addToWhitelist(newIpOrCidr.trim());
    if (success) {
      message.success(t("settings.access.addedToWhitelist", { ip: newIpOrCidr }));
      setNewIpOrCidr("");
    }
  };

  const handleRemove = async (ipOrCidr: string) => {
    const success = await removeFromWhitelist(ipOrCidr);
    if (success) {
      message.success(t("settings.access.removedFromWhitelist", { ip: ipOrCidr }));
    }
  };

  const handleAddTemp = async () => {
    if (!newTempIp.trim()) {
      message.warning(t("settings.access.enterIp"));
      return;
    }
    const success = await addTemporary(newTempIp.trim());
    if (success) {
      message.success(t("settings.access.addedToTempWhitelist", { ip: newTempIp }));
      setNewTempIp("");
    }
  };

  const handleRemoveTemp = async (ip: string) => {
    const success = await removeTemporary(ip);
    if (success) {
      message.success(t("settings.access.removedFromTempWhitelist", { ip }));
    }
  };

  const handleRemoveSessionDenied = async (ip: string) => {
    const success = await removeSessionDenied(ip);
    if (success) {
      message.success(t("settings.access.removedFromDenied", { ip }));
    }
  };

  const handleClearSessionDenied = async () => {
    const success = await clearSessionDenied();
    if (success) {
      message.success(t("settings.access.clearedDenied"));
    }
  };

  const handleModeChange = async (mode: AccessMode) => {
    const success = await setMode(mode);
    if (success) {
      message.success(t("settings.access.modeChanged", { mode }));
    }
  };

  const handleAllowLanChange = async (allow: boolean) => {
    const success = await setAllowLan(allow);
    if (success) {
      message.success(
        allow ? t("settings.access.lanEnabled") : t("settings.access.lanDisabled"),
      );
    }
  };

  const handleAddUserPassAccount = () => {
    setUserPassAccounts((current) => [
      ...current,
      {
        key: `new-${Date.now()}-${current.length}`,
        username: "",
        password: "",
        enabled: true,
        hasPassword: false,
        lastConnectedAt: null,
      },
    ]);
  };

  const handleRemoveUserPassAccount = (key: string) => {
    setUserPassAccounts((current) => current.filter((account) => account.key !== key));
  };

  const handleUpdateUserPassAccount = (
    key: string,
    field: keyof UserPassAccountDraft,
    value: string | boolean | null,
  ) => {
    setUserPassAccounts((current) =>
      current.map((account) =>
        account.key === key ? { ...account, [field]: value } : account,
      ),
    );
  };

  const handleSaveUserPassConfig = async () => {
    if (
      userPassAccounts.some(
        (account) => !account.username.trim() || (!account.hasPassword && !account.password.trim()),
      )
    ) {
      message.warning(t("settings.access.accountRequired"));
      return;
    }
    const success = await setUserPassConfig(
      userPassEnabled,
      userPassAccounts.map((account) => ({
        username: account.username.trim(),
        password: account.password.trim() ? account.password.trim() : undefined,
        enabled: account.enabled,
      })),
      loopbackRequiresAuth,
    );
    if (success) {
      setUserPassAccounts((current) =>
        current.map((account) => ({
          ...account,
          password: "",
          hasPassword: true,
        })),
      );
      message.success(t("settings.access.userPassUpdated"));
    }
  };

  const whitelistColumns: ColumnsType<{ ip: string }> = [
    {
      title: "IP / CIDR",
      dataIndex: "ip",
      key: "ip",
      render: (ip: string) => <Text code>{ip}</Text>,
    },
    {
      title: t("common.actions"),
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title={t("settings.access.removeFromWhitelist")}
          description={t("settings.access.removeIpConfirm", { ip: record.ip })}
          onConfirm={() => handleRemove(record.ip)}
          okText={t("common.yes")}
          cancelText={t("common.no")}
        >
          <Tooltip title={t("common.remove")}>
            <Button type="text" size="small" danger icon={<DeleteOutlined />} />
          </Tooltip>
        </Popconfirm>
      ),
    },
  ];

  const tempColumns: ColumnsType<{ ip: string }> = [
    {
      title: t("settings.access.ipAddress"),
      dataIndex: "ip",
      key: "ip",
      render: (ip: string) => <Text code>{ip}</Text>,
    },
    {
      title: t("common.actions"),
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title={t("settings.access.removeFromTempWhitelist")}
          description={t("settings.access.removeIpConfirm", { ip: record.ip })}
          onConfirm={() => handleRemoveTemp(record.ip)}
          okText={t("common.yes")}
          cancelText={t("common.no")}
        >
          <Tooltip title={t("common.remove")}>
            <Button type="text" size="small" danger icon={<DeleteOutlined />} />
          </Tooltip>
        </Popconfirm>
      ),
    },
  ];

  const sessionDeniedColumns: ColumnsType<{ ip: string }> = [
    {
      title: t("settings.access.ipAddress"),
      dataIndex: "ip",
      key: "ip",
      render: (ip: string) => <Text code>{ip}</Text>,
    },
    {
      title: t("common.actions"),
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title={t("settings.access.removeFromDenied")}
          description={t("settings.access.allowAgainConfirm", { ip: record.ip })}
          onConfirm={() => handleRemoveSessionDenied(record.ip)}
          okText={t("common.yes")}
          cancelText={t("common.no")}
        >
          <Tooltip title={t("common.remove")}>
            <Button type="text" size="small" danger icon={<DeleteOutlined />} />
          </Tooltip>
        </Popconfirm>
      ),
    },
  ];

  if (!status) {
    return (
      <Alert
        type="warning"
        message={t("settings.access.notConfigured")}
        description={t("settings.access.notConfiguredDescription")}
        showIcon
      />
    );
  }

  return (
    <div data-testid="settings-access-tab">
      <Row justify="space-between" align="middle" style={{ marginBottom: 16 }}>
        <Col>
          <Space>
            <Tag color={getModeColor(status.mode)} icon={getModeIcon(status.mode)}>
              {accessModeOptions.find((o) => o.value === status.mode)?.label ||
                status.mode}
            </Tag>
            {status.allow_lan && (status.mode === "whitelist" || status.mode === "interactive") && <Tag color="cyan">{t("settings.access.lanAllowed")}</Tag>}
          </Space>
        </Col>
        <Col>
          <Button icon={<ReloadOutlined />} onClick={() => fetchStatus()} data-testid="settings-access-refresh-button">
            {t("common.refresh")}
          </Button>
        </Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>{t("settings.access.settingsTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Row gutter={[16, 16]}>
              <Col span={24}>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Text type="secondary">{t("settings.access.mode")}</Text>
                  <Select
                    value={status.mode}
                    onChange={handleModeChange}
                    style={{ width: "100%" }}
                    data-testid="settings-access-mode-select"
                    options={accessModeOptions.map((o) => ({
                      value: o.value,
                      label: (
                        <Space>
                          {getModeIcon(o.value)}
                          <span>{o.label}</span>
                        </Space>
                      ),
                    }))}
                  />
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {accessModeOptions.find((o) => o.value === status.mode)
                      ?.description}
                  </Text>
                </Space>
              </Col>
              {(status.mode === "whitelist" || status.mode === "interactive") && (
              <Col span={24}>
                <Divider style={{ margin: "8px 0" }} />
                <Space>
                  <Switch
                    checked={status.allow_lan}
                    onChange={handleAllowLanChange}
                    data-testid="settings-access-allow-lan"
                  />
                  <Text>{t("settings.access.allowLan")}</Text>
                </Space>
                <br />
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {t("settings.access.allowLanHint")}
                </Text>
              </Col>
              )}
              <Col span={24}>
                <Divider style={{ margin: "8px 0" }} />
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Space>
                    <Switch
                      checked={userPassEnabled}
                      onChange={setUserPassEnabled}
                      data-testid="settings-access-userpass-enabled"
                    />
                    <Text>{t("settings.access.enableUserPass")}</Text>
                  </Space>
                  <Space>
                    <Switch
                      checked={loopbackRequiresAuth}
                      onChange={setLoopbackRequiresAuth}
                      data-testid="settings-access-loopback-requires-auth"
                    />
                    <Text>{t("settings.access.requireLocalhostAuth")}</Text>
                  </Space>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("settings.access.requireLocalhostAuthHint")}
                  </Text>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {t("settings.access.userPassHint")}
                  </Text>
                  <Space direction="vertical" style={{ width: "100%" }} size="middle">
                    {userPassAccounts.map((account) => (
                      <Card key={account.key} size="small">
                        <Space direction="vertical" size={8} style={{ width: "100%" }}>
                          <div
                            style={{
                              display: "flex",
                              alignItems: "flex-start",
                              justifyContent: "space-between",
                              gap: 12,
                            }}
                          >
                            <Text strong>{t("settings.access.account")}</Text>
                            <Space size={10} align="start" style={{ flexShrink: 0 }}>
                              <Space direction="vertical" size={2} align="end">
                                <Space size={6}>
                                  <Text type="secondary" style={{ fontSize: 12 }}>
                                    {t("common.enabled")}
                                  </Text>
                                  <Switch
                                    size="small"
                                    checked={account.enabled}
                                    onChange={(checked) =>
                                      handleUpdateUserPassAccount(account.key, "enabled", checked)
                                    }
                                  />
                                </Space>
                                <Text type="secondary" style={{ fontSize: 12 }}>
                                  {t("settings.access.lastConnected", { time: account.lastConnectedAt ?? "-" })}
                                </Text>
                              </Space>
                              <Button
                                type="text"
                                danger
                                size="small"
                                icon={<DeleteOutlined />}
                                onClick={() => handleRemoveUserPassAccount(account.key)}
                                aria-label={t("settings.access.removeAccount")}
                              />
                            </Space>
                          </div>
                          <Input
                            placeholder={t("settings.access.username")}
                            value={account.username}
                            onChange={(e) =>
                              handleUpdateUserPassAccount(account.key, "username", e.target.value)
                            }
                            data-testid={`settings-access-userpass-username-${account.key}`}
                          />
                          <Input.Password
                            placeholder={
                              account.hasPassword
                                ? t("settings.access.keepPassword")
                                : t("settings.access.password")
                            }
                            value={account.password}
                            onChange={(e) =>
                              handleUpdateUserPassAccount(account.key, "password", e.target.value)
                            }
                            data-testid={`settings-access-userpass-password-${account.key}`}
                          />
                        </Space>
                      </Card>
                    ))}
                    <Space>
                      <Button
                        icon={<PlusOutlined />}
                        onClick={handleAddUserPassAccount}
                        data-testid="settings-access-userpass-add-button"
                      >
                        {t("settings.access.addAccount")}
                      </Button>
                      <Button
                        type="primary"
                        onClick={handleSaveUserPassConfig}
                        data-testid="settings-access-userpass-save-button"
                      >
                        {t("settings.access.saveUserPass")}
                      </Button>
                    </Space>
                  </Space>
                </Space>
              </Col>
            </Row>
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>{t("settings.access.permanentWhitelist", { count: status.whitelist.length })}</span>
              </Space>
            }
            size="small"
            extra={
              <Space.Compact>
                <Input
                  placeholder={t("settings.access.ipOrCidrPlaceholder")}
                  value={newIpOrCidr}
                  onChange={(e) => setNewIpOrCidr(e.target.value)}
                  onPressEnter={handleAdd}
                  style={{ width: 200 }}
                  size="small"
                  data-testid="settings-whitelist-input"
                />
                <Button
                  type="primary"
                  icon={<PlusOutlined />}
                  onClick={handleAdd}
                  size="small"
                  data-testid="settings-whitelist-add-button"
                >
                  {t("common.add")}
                </Button>
              </Space.Compact>
            }
          >
            <Table
              columns={whitelistColumns}
              dataSource={status.whitelist.map((ip) => ({ ip }))}
              rowKey="ip"
              loading={loading}
              size="small"
              pagination={{ pageSize: 10, showSizeChanger: false }}
              data-testid="settings-whitelist-table"
            />
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>
                  {t("settings.access.temporaryWhitelist", { count: status.temporary_whitelist.length })}
                </span>
              </Space>
            }
            size="small"
            extra={
              <Space.Compact>
                <Input
                  placeholder={t("settings.access.temporaryIp")}
                  value={newTempIp}
                  onChange={(e) => setNewTempIp(e.target.value)}
                  onPressEnter={handleAddTemp}
                  style={{ width: 200 }}
                  size="small"
                  data-testid="settings-temp-whitelist-input"
                />
                <Button
                  type="primary"
                  icon={<PlusOutlined />}
                  onClick={handleAddTemp}
                  size="small"
                  data-testid="settings-temp-whitelist-add-button"
                >
                  {t("common.add")}
                </Button>
              </Space.Compact>
            }
          >
            <Table
              columns={tempColumns}
              dataSource={status.temporary_whitelist.map((ip) => ({ ip }))}
              rowKey="ip"
              loading={loading}
              size="small"
              pagination={{ pageSize: 10, showSizeChanger: false }}
              data-testid="settings-temp-whitelist-table"
            />
          </Card>
        </Col>

        {status.session_denied.length > 0 && (
        <Col xs={24}>
          <Card
            title={
              <Space>
                <StopOutlined />
                <span>
                  {t("settings.access.sessionDenied", { count: status.session_denied.length })}
                </span>
              </Space>
            }
            size="small"
            extra={
              <Popconfirm
                title={t("settings.access.clearDeniedTitle")}
                description={t("settings.access.clearDeniedDescription")}
                onConfirm={handleClearSessionDenied}
                okText={t("common.yes")}
                cancelText={t("common.no")}
              >
                <Button
                  icon={<ClearOutlined />}
                  size="small"
                  danger
                  data-testid="settings-session-denied-clear-button"
                >
                  {t("settings.access.clearAll")}
                </Button>
              </Popconfirm>
            }
          >
            <Alert
              type="info"
              message={t("settings.access.sessionDeniedHint")}
              style={{ marginBottom: 12 }}
              showIcon
            />
            <Table
              columns={sessionDeniedColumns}
              dataSource={status.session_denied.map((ip) => ({ ip }))}
              rowKey="ip"
              loading={loading}
              size="small"
              pagination={{ pageSize: 10, showSizeChanger: false }}
              data-testid="settings-session-denied-table"
            />
          </Card>
        </Col>
        )}
      </Row>
    </div>
  );
}
