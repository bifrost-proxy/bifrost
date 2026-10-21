import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Col,
  Descriptions,
  Divider,
  Input,
  Popconfirm,
  Row,
  Space,
  Switch,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import {
  CheckCircleOutlined,
  CloseCircleOutlined,
  KeyOutlined,
  LockOutlined,
  ReloadOutlined,
  SafetyOutlined,
  StopOutlined,
  UserOutlined,
  WarningOutlined,
} from "@ant-design/icons";
import {
  changeAdminPassword,
  fetchAdminAuthStatus,
  fetchLoginAudit,
  revokeAllSessions,
  setRemoteAccess,
  type AdminAuthStatus,
  type LoginAuditEntry,
} from "../../../services/adminAuth";
import { useI18n, type TFunction } from "../../../i18n";

const { Text } = Typography;

const getAuditColumns = (t: TFunction): ColumnsType<LoginAuditEntry> => [
  {
    title: t("common.status"),
    dataIndex: "success",
    key: "success",
    width: 80,
    render: (success: boolean) =>
      success ? (
        <Tag icon={<CheckCircleOutlined />} color="success">{t("common.ok")}</Tag>
      ) : (
        <Tag icon={<CloseCircleOutlined />} color="error">{t("common.failed")}</Tag>
      ),
  },
  {
    title: t("settings.remote.time"),
    dataIndex: "ts",
    key: "ts",
    width: 180,
    render: (ts: number) => {
      const d = new Date(ts * 1000);
      return (
        <Tooltip title={d.toISOString()}>
          {d.toLocaleString()}
        </Tooltip>
      );
    },
  },
  {
    title: "IP",
    dataIndex: "ip",
    key: "ip",
    width: 160,
  },
  {
    title: t("settings.access.username"),
    dataIndex: "username",
    key: "username",
    width: 120,
  },
  {
    title: "User Agent",
    dataIndex: "ua",
    key: "ua",
    ellipsis: { showTitle: false },
    render: (ua: string) => (
      <Tooltip title={ua}>
        <Text type="secondary" style={{ fontSize: 12 }}>
          {ua || "-"}
        </Text>
      </Tooltip>
    ),
  },
];

export default function RemoteAccessTab() {
  const { t } = useI18n();
  const auditColumns = useMemo(() => getAuditColumns(t), [t]);
  const [status, setStatus] = useState<AdminAuthStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [auditItems, setAuditItems] = useState<LoginAuditEntry[]>([]);
  const [auditTotal, setAuditTotal] = useState(0);
  const [auditPage, setAuditPage] = useState(1);
  const [auditLoading, setAuditLoading] = useState(false);
  const auditPageSize = 10;

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const s = await fetchAdminAuthStatus();
      setStatus(s);
      setUsername(s.username || "admin");
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAudit = useCallback(async (page = 1) => {
    setAuditLoading(true);
    try {
      const res = await fetchLoginAudit(auditPageSize, (page - 1) * auditPageSize);
      setAuditItems(res.items);
      setAuditTotal(res.total);
      setAuditPage(page);
    } catch {
      // ignore
    } finally {
      setAuditLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    void refreshAudit(1);
  }, [refresh, refreshAudit]);

  const handleToggleRemoteAccess = async (enabled: boolean) => {
    setLoading(true);
    try {
      const s = await setRemoteAccess(enabled);
      setStatus(s);
      message.success(
        enabled ? t("settings.remote.enabledMsg") : t("settings.remote.disabledMsg"),
      );
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("settings.remote.toggleFailed"),
      );
    } finally {
      setLoading(false);
    }
  };

  const handleChangePassword = async () => {
    if (!password.trim()) {
      message.warning(t("settings.remote.enterPassword"));
      return;
    }
    const minLen = status?.min_password_length ?? 6;
    if (password.length < minLen) {
      message.warning(t("settings.remote.passwordMinLength", { count: minLen }));
      return;
    }
    const hasLetter = /[a-zA-Z]/.test(password);
    const hasDigit = /\d/.test(password);
    if (!hasLetter || !hasDigit) {
      message.warning(t("settings.remote.passwordLettersDigits"));
      return;
    }
    if (password !== confirmPassword) {
      message.warning(t("settings.remote.passwordMismatch"));
      return;
    }
    setLoading(true);
    try {
      await changeAdminPassword(password, username);
      message.success(t("settings.remote.passwordUpdated"));
      setPassword("");
      setConfirmPassword("");
      await refresh();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("settings.remote.changePasswordFailed"),
      );
    } finally {
      setLoading(false);
    }
  };

  const handleRevokeAll = async () => {
    setLoading(true);
    try {
      await revokeAllSessions();
      message.success(t("settings.remote.sessionsRevoked"));
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : t("settings.remote.revokeFailed"),
      );
    } finally {
      setLoading(false);
    }
  };

  return (
    <div data-testid="settings-remote-access-tab">
      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>{t("settings.remote.statusTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Descriptions column={1} size="small">
              <Descriptions.Item label={t("settings.tabRemoteAccess")}>
                {status?.remote_access_enabled ? (
                  <Tag color="green">{t("common.enabled")}</Tag>
                ) : (
                  <Tag color="default">{t("common.disabled")}</Tag>
                )}
              </Descriptions.Item>
              <Descriptions.Item label={t("settings.remote.adminUsername")}>
                <Text code>{status?.username || "admin"}</Text>
              </Descriptions.Item>
              <Descriptions.Item label={t("settings.access.password")}>
                {status?.has_password ? (
                  <Tag color="green">{t("settings.remote.set")}</Tag>
                ) : (
                  <Tag color="orange">{t("settings.remote.notSet")}</Tag>
                )}
              </Descriptions.Item>
              <Descriptions.Item label={t("settings.remote.failedAttempts")}>
                {(status?.failed_attempts ?? 0) > 0 ? (
                  <Tag color="red">{status?.failed_attempts}/{status?.max_attempts ?? 5}</Tag>
                ) : (
                  <Tag color="green">0/{status?.max_attempts ?? 5}</Tag>
                )}
              </Descriptions.Item>
            </Descriptions>

            {status?.locked_out ? (
              <Alert
                type="error"
                icon={<WarningOutlined />}
                message={t("settings.remote.lockoutTitle")}
                description={t("settings.remote.lockoutDescription")}
                showIcon
                style={{ margin: "12px 0" }}
              />
            ) : null}

            <Divider style={{ margin: "12px 0" }} />

            <Space>
              <Switch
                checked={status?.remote_access_enabled ?? false}
                onChange={handleToggleRemoteAccess}
                loading={loading}
                data-testid="settings-remote-access-toggle"
              />
              <Text>{t("settings.remote.enable")}</Text>
            </Space>
            <br />
            <Text type="secondary" style={{ fontSize: 12 }}>
              {t("settings.remote.enableHint")}
            </Text>
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <KeyOutlined />
                <span>{t("settings.remote.credentialsTitle")}</span>
              </Space>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <div>
                <Text type="secondary" style={{ marginBottom: 4, display: "block" }}>
                  {t("settings.access.username")}
                </Text>
                <Input
                  prefix={<UserOutlined />}
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder="admin"
                  data-testid="settings-remote-username"
                />
              </div>
              <div>
                <Text type="secondary" style={{ marginBottom: 4, display: "block" }}>
                  {t("settings.remote.newPassword")}
                </Text>
                <Input.Password
                  prefix={<LockOutlined />}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={t("settings.remote.newPasswordPlaceholder")}
                  data-testid="settings-remote-password"
                />
                <Text type="secondary" style={{ fontSize: 11 }}>
                  {t("settings.remote.passwordRule", { count: status?.min_password_length ?? 6 })}
                </Text>
              </div>
              <div>
                <Text type="secondary" style={{ marginBottom: 4, display: "block" }}>
                  {t("settings.remote.confirmPassword")}
                </Text>
                <Input.Password
                  prefix={<LockOutlined />}
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  placeholder={t("settings.remote.confirmPasswordPlaceholder")}
                  onPressEnter={handleChangePassword}
                  data-testid="settings-remote-confirm-password"
                />
              </div>
              <Button
                type="primary"
                onClick={handleChangePassword}
                loading={loading}
                data-testid="settings-remote-save-password"
              >
                {t("settings.remote.saveCredentials")}
              </Button>
            </Space>

            {!status?.has_password && (
              <Alert
                type="info"
                message={t("settings.remote.setPasswordHint")}
                showIcon
                style={{ marginTop: 12 }}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <StopOutlined />
                <span>{t("settings.remote.sessionsTitle")}</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshAudit(auditPage)}
                loading={auditLoading}
              >
                {t("common.refresh")}
              </Button>
            }
            size="small"
          >
            <Space direction="vertical" style={{ width: "100%" }}>
              <Text type="secondary">
                {t("settings.remote.sessionsHint")}
              </Text>
              <Table<LoginAuditEntry>
                columns={auditColumns}
                dataSource={auditItems}
                rowKey="id"
                size="small"
                loading={auditLoading}
                pagination={{
                  current: auditPage,
                  pageSize: auditPageSize,
                  total: auditTotal,
                  showTotal: (total) => t("settings.remote.records", { total }),
                  showSizeChanger: false,
                  onChange: (page) => void refreshAudit(page),
                }}
                data-testid="settings-remote-audit-table"
              />
              <Divider style={{ margin: "4px 0" }} />
              <Popconfirm
                title={t("settings.remote.revokeTitle")}
                description={t("settings.remote.revokeDescription")}
                onConfirm={handleRevokeAll}
                okText={t("settings.remote.revoke")}
                cancelText={t("common.cancel")}
              >
                <Button
                  danger
                  loading={loading}
                  data-testid="settings-remote-revoke-all"
                >
                  {t("settings.remote.revokeAll")}
                </Button>
              </Popconfirm>
            </Space>
          </Card>
        </Col>
      </Row>
    </div>
  );
}
