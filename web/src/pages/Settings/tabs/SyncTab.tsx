import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Input,
  Modal,
  Radio,
  Space,
  Switch,
  Tag,
  Typography,
} from "antd";
import {
  CloudOutlined,
  LinkOutlined,
  LoginOutlined,
  LogoutOutlined,
  ReloadOutlined,
} from "@ant-design/icons";
import type { SyncProviderStatus, SyncStatus } from "../../../api/sync";

const { Text } = Typography;

interface SyncTabProps {
  syncStatus: SyncStatus | null;
  syncLoading: boolean;
  remoteBaseUrlDraft: string;
  setRemoteBaseUrlDraft: (value: string) => void;
  onToggleEnabled: (enabled: boolean) => void;
  onToggleAutoSync: (enabled: boolean) => void;
  onSaveRemoteBaseUrl: () => void;
  onSignIn: () => void;
  onProviderSignIn?: (provider: SyncProviderStatus) => void;
  onSignOut: () => void;
  onRunSync: () => void;
}

function renderStatusTag(syncStatus: SyncStatus | null) {
  if (!syncStatus) {
    return <Tag>Unknown</Tag>;
  }
  switch (syncStatus.reason) {
    case "ready":
      return <Tag color="green">Ready</Tag>;
    case "syncing":
      return <Tag color="processing">Syncing</Tag>;
    case "unreachable":
      return <Tag color="orange">Offline</Tag>;
    case "unauthorized":
      return <Tag color="gold">Sign in required</Tag>;
    case "error":
      return <Tag color="red">Error</Tag>;
    default:
      return <Tag>{syncStatus.reason}</Tag>;
  }
}

function renderLastAction(syncStatus: SyncStatus | null) {
  switch (syncStatus?.last_sync_action) {
    case "local_pushed":
      return "Local changes pushed to remote";
    case "remote_pulled":
      return "Newer remote changes pulled into local";
    case "bidirectional":
      return "Local and remote changes exchanged";
    case "no_change":
      return "No changes detected";
    default:
      return "No sync result yet";
  }
}

function capabilityTags(provider: SyncProviderStatus) {
  return (
    <Space size={[4, 4]} wrap>
      <Tag color={provider.capabilities.remote_invoke ? "blue" : "default"}>
        Remote Invoke
      </Tag>
      <Tag color={provider.capabilities.rules_sync ? "green" : "default"}>
        Rules Sync
      </Tag>
      <Tag color={provider.capabilities.config_sync ? "cyan" : "default"}>
        Config Sync
      </Tag>
    </Space>
  );
}

function providerStatusTag(provider: SyncProviderStatus) {
  if (provider.connected) {
    return <Tag color="green">Connected</Tag>;
  }
  if (provider.enabled && provider.reachable) {
    return <Tag color="gold">Sign in required</Tag>;
  }
  if (provider.enabled && !provider.reachable) {
    return <Tag color="orange">Offline</Tag>;
  }
  return <Tag>Not connected</Tag>;
}

const fallbackProviders: SyncProviderStatus[] = [
  {
    id: "bytedance_internal",
    name: "ByteDance Internal",
    description: "Internal trusted sync and Remote Invoke provider.",
    remote_base_url: "https://bifrost.bytedance.net",
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    user: null,
    capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
  {
    id: "bifrost_cloud",
    name: "Bifrost Cloud",
    description: "Custom Bifrost sync service for teams and self-hosting.",
    remote_base_url: "https://sync.bifrostproxy.dev",
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    user: null,
    capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
  {
    id: "github_gist",
    name: "GitHub Gist",
    description: "Public GitHub Gist-backed portable sync provider.",
    remote_base_url: null,
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    user: null,
    capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
];

export default function SyncTab({
  syncStatus,
  syncLoading,
  remoteBaseUrlDraft,
  setRemoteBaseUrlDraft,
  onToggleEnabled,
  onToggleAutoSync,
  onSaveRemoteBaseUrl,
  onSignIn,
  onProviderSignIn,
  onSignOut,
  onRunSync,
}: SyncTabProps) {
  const providers = syncStatus?.providers?.length
    ? syncStatus.providers
    : fallbackProviders;
  const [firstRunOpen, setFirstRunOpen] = useState(false);
  const [selectedProviderId, setSelectedProviderId] = useState(providers[0]?.id);
  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === selectedProviderId) || providers[0],
    [providers, selectedProviderId],
  );

  useEffect(() => {
    if (syncStatus?.first_run_prompt_required) {
      setFirstRunOpen(true);
    }
  }, [syncStatus?.first_run_prompt_required]);

  useEffect(() => {
    if (!providers.some((provider) => provider.id === selectedProviderId)) {
      setSelectedProviderId(providers[0]?.id);
    }
  }, [providers, selectedProviderId]);

  const handleProviderSignIn = (provider: SyncProviderStatus) => {
    if (provider.remote_base_url) {
      setRemoteBaseUrlDraft(provider.remote_base_url);
    }
    onProviderSignIn?.(provider);
    if (!onProviderSignIn) {
      onSignIn();
    }
  };

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      <Alert
        showIcon
        type={syncStatus?.reachable ? "info" : "warning"}
        message="Remote sync is a pluggable capability"
        description="Rules continue to work locally at all times. Sync only activates when the configured Bifrost service is reachable and a valid login session exists."
      />

      <div
        data-testid="settings-sync-provider-grid"
        style={{
          display: "grid",
          gap: 16,
          gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))",
          maxWidth: 1080,
          width: "100%",
        }}
      >
        {providers.map((provider) => (
          <Card
            key={provider.id}
            size="small"
            data-testid={`settings-sync-provider-card-${provider.id}`}
            title={
              <Space>
                <CloudOutlined />
                <span>{provider.name}</span>
              </Space>
            }
            extra={providerStatusTag(provider)}
          >
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Text type="secondary">{provider.description}</Text>
              {capabilityTags(provider)}
              <Descriptions
                size="small"
                column={1}
                items={[
                  {
                    key: "account",
                    label: "Account",
                    children:
                      provider.user?.user_id ||
                      (provider.connected ? "Signed in" : "Not signed in"),
                  },
                  {
                    key: "remote",
                    label: "Remote",
                    children: provider.remote_base_url || "GitHub Gist",
                  },
                  {
                    key: "invoke",
                    label: "Remote Invoke",
                    children: provider.capabilities.remote_invoke
                      ? provider.remote_invoke_registered
                        ? "Registered"
                        : "Supported"
                      : "Not supported",
                  },
                ]}
              />
              <Space wrap>
                <Button
                  type={provider.connected ? "default" : "primary"}
                  icon={<LoginOutlined />}
                  onClick={() => handleProviderSignIn(provider)}
                  disabled={provider.id === "github_gist" || syncLoading}
                  loading={syncLoading}
                  data-testid={`settings-sync-provider-login-${provider.id}`}
                >
                  {provider.connected ? "Reconnect" : "Sign In"}
                </Button>
                {provider.connected ? (
                  <Button
                    icon={<LogoutOutlined />}
                    onClick={onSignOut}
                    loading={syncLoading}
                    data-testid={`settings-sync-provider-logout-${provider.id}`}
                  >
                    Sign Out
                  </Button>
                ) : null}
              </Space>
            </Space>
          </Card>
        ))}
      </div>

      <Card title="Remote Sync" size="small">
        <Space direction="vertical" size={16} style={{ width: "100%" }}>
          <Descriptions
            size="small"
            column={1}
            items={[
              {
                key: "status",
                label: "Status",
                children: renderStatusTag(syncStatus),
              },
              {
                key: "last-sync",
                label: "Last Sync",
                children: syncStatus?.last_sync_at || "Never",
              },
              {
                key: "session",
                label: "Session",
                children: (
                  <span data-testid="settings-sync-session">
                    {syncStatus?.user?.user_id || "Not signed in"}
                  </span>
                ),
              },
              {
                key: "last-sync-action",
                label: "Last Result",
                children: (
                  <span data-testid="settings-sync-last-action">
                    {renderLastAction(syncStatus)}
                  </span>
                ),
              },
            ]}
          />

          {syncStatus?.last_error ? (
            <Alert
              showIcon
              type="error"
              message="Last sync error"
              description={syncStatus.last_error}
            />
          ) : null}

          <Space align="center">
            <Text strong>Enable sync</Text>
            <Switch
              checked={syncStatus?.enabled ?? false}
              loading={syncLoading}
              onChange={onToggleEnabled}
              data-testid="settings-sync-enable-switch"
            />
          </Space>

          <Space align="center">
            <Text strong>Auto sync</Text>
            <Switch
              checked={syncStatus?.auto_sync ?? false}
              loading={syncLoading}
              onChange={onToggleAutoSync}
              disabled={!(syncStatus?.enabled ?? false)}
              data-testid="settings-sync-auto-switch"
            />
          </Space>

          <Space.Compact style={{ width: "100%" }}>
            <Input
              value={remoteBaseUrlDraft}
              onChange={(event) => setRemoteBaseUrlDraft(event.target.value)}
              placeholder="https://bifrost.bytedance.net"
              prefix={<LinkOutlined />}
              data-testid="settings-sync-remote-url-input"
            />
            <Button
              type="primary"
              onClick={onSaveRemoteBaseUrl}
              loading={syncLoading}
              data-testid="settings-sync-remote-url-save"
            >
              Save
            </Button>
          </Space.Compact>

          <Space wrap>
            <Button
              type="primary"
              icon={<LoginOutlined />}
              onClick={onSignIn}
              disabled={!(syncStatus?.enabled ?? false)}
              loading={syncLoading}
              data-testid="settings-sync-sign-in"
            >
              Sign In
            </Button>
            <Button
              icon={<LogoutOutlined />}
              onClick={onSignOut}
              disabled={!syncStatus?.has_session}
              loading={syncLoading}
              data-testid="settings-sync-sign-out"
            >
              Sign Out
            </Button>
            <Button
              icon={<ReloadOutlined />}
              onClick={onRunSync}
              disabled={!syncStatus?.authorized}
              loading={syncLoading || syncStatus?.syncing}
              data-testid="settings-sync-run-now"
            >
              Sync Now
            </Button>
          </Space>
        </Space>
      </Card>

      <Modal
        title="Choose a sync service"
        open={firstRunOpen}
        onCancel={() => setFirstRunOpen(false)}
        footer={[
          <Button key="cancel" onClick={() => setFirstRunOpen(false)}>
            Not now
          </Button>,
          <Button
            key="start"
            type="primary"
            icon={<LoginOutlined />}
            disabled={!selectedProvider || selectedProvider.id === "github_gist"}
            loading={syncLoading}
            onClick={() => {
              if (selectedProvider) {
                handleProviderSignIn(selectedProvider);
                setFirstRunOpen(false);
              }
            }}
            data-testid="settings-sync-first-run-start"
          >
            Start
          </Button>,
        ]}
        data-testid="settings-sync-first-run-modal"
      >
        <Radio.Group
          value={selectedProviderId}
          onChange={(event) => setSelectedProviderId(event.target.value)}
          style={{ width: "100%" }}
        >
          <Space direction="vertical" size={12} style={{ width: "100%" }}>
            {providers.map((provider) => (
              <Radio key={provider.id} value={provider.id}>
                <Space direction="vertical" size={2}>
                  <Text strong>{provider.name}</Text>
                  {capabilityTags(provider)}
                </Space>
              </Radio>
            ))}
          </Space>
        </Radio.Group>
      </Modal>
    </Space>
  );
}
