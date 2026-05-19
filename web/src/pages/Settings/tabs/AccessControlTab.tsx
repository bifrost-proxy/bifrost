import { useEffect, useMemo, useState } from "react";
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
import type { AccessMode } from "../../../types";
import { notifyApiBusinessError } from "../../../api/client";

const { Text } = Typography;

const accessModeOptions: {
  value: AccessMode;
  label: string;
  description: string;
}[] = [
  {
    value: "allow_all",
    label: "Allow All",
    description: "Allow all connections (no restriction)",
  },
  {
    value: "local_only",
    label: "Local Only",
    description: "Only allow localhost connections (127.0.0.1, ::1)",
  },
  {
    value: "whitelist",
    label: "Whitelist",
    description: "Only allow whitelisted IPs/CIDRs",
  },
  {
    value: "interactive",
    label: "Interactive",
    description: "Prompt for unknown IPs (not implemented)",
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

export default function AccessControlTab() {
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

  const derivedUserPass = useMemo(() => {
    if (!status) {
      return null;
    }
    return {
      enabled: status.userpass.enabled,
      loopbackRequiresAuth: status.userpass.loopback_requires_auth ?? false,
      accounts: status.userpass.accounts.map((account) => ({
        key: account.username,
        username: account.username,
        password: "",
        enabled: account.enabled,
        hasPassword: account.has_password,
        lastConnectedAt: account.last_connected_at,
      })),
    };
  }, [status]);

  const [userPassEnabled, setUserPassEnabled] = useState(false);
  const [loopbackRequiresAuth, setLoopbackRequiresAuth] = useState(false);
  const [userPassAccounts, setUserPassAccounts] = useState<UserPassAccountDraft[]>([]);
  const [lastSyncedStatus, setLastSyncedStatus] = useState(status);

  if (derivedUserPass && status !== lastSyncedStatus) {
    setLastSyncedStatus(status);
    setUserPassEnabled(derivedUserPass.enabled);
    setLoopbackRequiresAuth(derivedUserPass.loopbackRequiresAuth);
    setUserPassAccounts(derivedUserPass.accounts);
  }

  const handleAdd = async () => {
    if (!newIpOrCidr.trim()) {
      message.warning("Please enter an IP address or CIDR");
      return;
    }
    const success = await addToWhitelist(newIpOrCidr.trim());
    if (success) {
      message.success(`Added ${newIpOrCidr} to whitelist`);
      setNewIpOrCidr("");
    }
  };

  const handleRemove = async (ipOrCidr: string) => {
    const success = await removeFromWhitelist(ipOrCidr);
    if (success) {
      message.success(`Removed ${ipOrCidr} from whitelist`);
    }
  };

  const handleAddTemp = async () => {
    if (!newTempIp.trim()) {
      message.warning("Please enter an IP address");
      return;
    }
    const success = await addTemporary(newTempIp.trim());
    if (success) {
      message.success(`Added ${newTempIp} to temporary whitelist`);
      setNewTempIp("");
    }
  };

  const handleRemoveTemp = async (ip: string) => {
    const success = await removeTemporary(ip);
    if (success) {
      message.success(`Removed ${ip} from temporary whitelist`);
    }
  };

  const handleRemoveSessionDenied = async (ip: string) => {
    const success = await removeSessionDenied(ip);
    if (success) {
      message.success(`Removed ${ip} from denied list`);
    }
  };

  const handleClearSessionDenied = async () => {
    const success = await clearSessionDenied();
    if (success) {
      message.success("Cleared all denied IPs");
    }
  };

  const handleModeChange = async (mode: AccessMode) => {
    const success = await setMode(mode);
    if (success) {
      message.success(`Access mode changed to ${mode}`);
    }
  };

  const handleAllowLanChange = async (allow: boolean) => {
    const success = await setAllowLan(allow);
    if (success) {
      message.success(`LAN access ${allow ? "enabled" : "disabled"}`);
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
      message.warning("New accounts require both username and password");
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
      message.success("Updated user/password proxy authentication");
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
      title: "Actions",
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title="Remove from whitelist"
          description={`Remove ${record.ip}?`}
          onConfirm={() => handleRemove(record.ip)}
          okText="Yes"
          cancelText="No"
        >
          <Tooltip title="Remove">
            <Button type="text" size="small" danger icon={<DeleteOutlined />} />
          </Tooltip>
        </Popconfirm>
      ),
    },
  ];

  const tempColumns: ColumnsType<{ ip: string }> = [
    {
      title: "IP Address",
      dataIndex: "ip",
      key: "ip",
      render: (ip: string) => <Text code>{ip}</Text>,
    },
    {
      title: "Actions",
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title="Remove from temporary whitelist"
          description={`Remove ${record.ip}?`}
          onConfirm={() => handleRemoveTemp(record.ip)}
          okText="Yes"
          cancelText="No"
        >
          <Tooltip title="Remove">
            <Button type="text" size="small" danger icon={<DeleteOutlined />} />
          </Tooltip>
        </Popconfirm>
      ),
    },
  ];

  const sessionDeniedColumns: ColumnsType<{ ip: string }> = [
    {
      title: "IP Address",
      dataIndex: "ip",
      key: "ip",
      render: (ip: string) => <Text code>{ip}</Text>,
    },
    {
      title: "Actions",
      key: "actions",
      width: 100,
      align: "center",
      render: (_, record) => (
        <Popconfirm
          title="Remove from denied list"
          description={`Allow ${record.ip} to connect again?`}
          onConfirm={() => handleRemoveSessionDenied(record.ip)}
          okText="Yes"
          cancelText="No"
        >
          <Tooltip title="Remove">
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
        message="Access Control Not Configured"
        description="The access control feature is not configured on the server. Start the proxy with --access-mode option to enable it."
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
            {status.allow_lan && (status.mode === "whitelist" || status.mode === "interactive") && <Tag color="cyan">LAN Allowed</Tag>}
          </Space>
        </Col>
        <Col>
          <Button icon={<ReloadOutlined />} onClick={() => fetchStatus()} data-testid="settings-access-refresh-button">
            Refresh
          </Button>
        </Col>
      </Row>

      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>Access Settings</span>
              </Space>
            }
            size="small"
          >
            <Row gutter={[16, 16]}>
              <Col span={24}>
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Text type="secondary">Access Mode</Text>
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
                  <Text>Allow LAN Connections</Text>
                </Space>
                <br />
                <Text type="secondary" style={{ fontSize: 12 }}>
                  When enabled, private network IPs (192.168.x.x, 10.x.x.x,
                  172.16-31.x.x) are allowed
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
                    <Text>Enable User/Password Auth</Text>
                  </Space>
                  <Space>
                    <Switch
                      checked={loopbackRequiresAuth}
                      onChange={setLoopbackRequiresAuth}
                      data-testid="settings-access-loopback-requires-auth"
                    />
                    <Text>Require Auth for Localhost</Text>
                  </Space>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    When enabled, loopback (127.0.0.1) connections also need username/password authentication.
                  </Text>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    When IP-based access does not pass, configured accounts can still authenticate over HTTP and SOCKS5.
                  </Text>
                  <Space direction="vertical" style={{ width: "100%" }} size="middle">
                    {userPassAccounts.map((account) => (
                      <Card key={account.key} size="small">
                        <Space direction="vertical" style={{ width: "100%" }}>
                          <Space wrap style={{ width: "100%", justifyContent: "space-between" }}>
                            <Text strong>Account</Text>
                            <Button
                              type="text"
                              danger
                              icon={<DeleteOutlined />}
                              onClick={() => handleRemoveUserPassAccount(account.key)}
                            >
                              Remove
                            </Button>
                          </Space>
                          <Input
                            placeholder="Username"
                            value={account.username}
                            onChange={(e) =>
                              handleUpdateUserPassAccount(account.key, "username", e.target.value)
                            }
                            data-testid={`settings-access-userpass-username-${account.key}`}
                          />
                          <Input.Password
                            placeholder={account.hasPassword ? "Leave blank to keep current password" : "Password"}
                            value={account.password}
                            onChange={(e) =>
                              handleUpdateUserPassAccount(account.key, "password", e.target.value)
                            }
                            data-testid={`settings-access-userpass-password-${account.key}`}
                          />
                          <Space>
                            <Switch
                              checked={account.enabled}
                              onChange={(checked) =>
                                handleUpdateUserPassAccount(account.key, "enabled", checked)
                              }
                            />
                            <Text>Account Enabled</Text>
                          </Space>
                          <Text type="secondary" style={{ fontSize: 12 }}>
                            Last Connected: {account.lastConnectedAt ?? "-"}
                          </Text>
                        </Space>
                      </Card>
                    ))}
                    <Space>
                      <Button
                        icon={<PlusOutlined />}
                        onClick={handleAddUserPassAccount}
                        data-testid="settings-access-userpass-add-button"
                      >
                        Add Account
                      </Button>
                      <Button
                        type="primary"
                        onClick={handleSaveUserPassConfig}
                        data-testid="settings-access-userpass-save-button"
                      >
                        Save User/Password Auth
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
                <span>Permanent Whitelist ({status.whitelist.length})</span>
              </Space>
            }
            size="small"
            extra={
              <Space.Compact>
                <Input
                  placeholder="IP or CIDR (e.g., 192.168.1.0/24)"
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
                  Add
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
                  Temporary Whitelist ({status.temporary_whitelist.length})
                </span>
              </Space>
            }
            size="small"
            extra={
              <Space.Compact>
                <Input
                  placeholder="Temporary IP"
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
                  Add
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
                  Session Denied IPs ({status.session_denied.length})
                </span>
              </Space>
            }
            size="small"
            extra={
              <Popconfirm
                title="Clear all denied IPs"
                description="This will allow all previously denied IPs to connect again."
                onConfirm={handleClearSessionDenied}
                okText="Yes"
                cancelText="No"
              >
                <Button
                  icon={<ClearOutlined />}
                  size="small"
                  danger
                  data-testid="settings-session-denied-clear-button"
                >
                  Clear All
                </Button>
              </Popconfirm>
            }
          >
            <Alert
              type="info"
              message="These IPs were denied during this session. They will be blocked until removed or the service restarts."
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
