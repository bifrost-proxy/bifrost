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
  Tag,
  Typography,
} from "antd";
import {
  CloudOutlined,
  GithubOutlined,
  LinkOutlined,
  LoginOutlined,
  LogoutOutlined,
} from "@ant-design/icons";
import type { SyncProviderStatus, SyncStatus } from "../../../api/sync";

const { Text } = Typography;

interface SyncTabProps {
  syncStatus: SyncStatus | null;
  syncLoading: boolean;
  onSignIn: () => void;
  onProviderSignIn?: (
    provider: SyncProviderStatus,
    options?: { token?: string },
  ) => void;
  onProviderRemoteBaseUrlSave?: (
    provider: SyncProviderStatus,
    remoteBaseUrl: string,
  ) => void;
  onSignOut: () => void;
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
  onSignIn,
  onProviderSignIn,
  onProviderRemoteBaseUrlSave,
  onSignOut,
}: SyncTabProps) {
  const providers = syncStatus?.providers?.length
    ? syncStatus.providers
    : fallbackProviders;
  const [firstRunOpen, setFirstRunOpen] = useState(false);
  const [selectedProviderId, setSelectedProviderId] = useState(providers[0]?.id);
  const bifrostCloudProvider = providers.find(
    (provider) => provider.id === "bifrost_cloud",
  );
  const [bifrostCloudUrlDraft, setBifrostCloudUrlDraft] = useState(
    bifrostCloudProvider?.remote_base_url || "https://sync.bifrostproxy.dev",
  );
  const [bifrostCloudUrlDirty, setBifrostCloudUrlDirty] = useState(false);
  const [githubTokenModalOpen, setGithubTokenModalOpen] = useState(false);
  const [githubTokenDraft, setGithubTokenDraft] = useState("");
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

  useEffect(() => {
    if (bifrostCloudUrlDirty) {
      return;
    }
    setBifrostCloudUrlDraft(
      bifrostCloudProvider?.remote_base_url || "https://sync.bifrostproxy.dev",
    );
  }, [bifrostCloudProvider?.remote_base_url, bifrostCloudUrlDirty]);

  const providerWithDraftUrl = (provider: SyncProviderStatus) => {
    if (provider.id !== "bifrost_cloud") {
      return provider;
    }
    return {
      ...provider,
      remote_base_url: bifrostCloudUrlDraft.trim() || provider.remote_base_url,
    };
  };

  const handleProviderSignIn = (provider: SyncProviderStatus) => {
    if (provider.id === "github_gist") {
      setGithubTokenModalOpen(true);
      return;
    }
    onProviderSignIn?.(providerWithDraftUrl(provider));
    if (!onProviderSignIn) {
      onSignIn();
    }
  };

  const handleGithubGistSignIn = () => {
    const githubProvider = providers.find((provider) => provider.id === "github_gist");
    if (!githubProvider || !githubTokenDraft.trim()) {
      return;
    }
    onProviderSignIn?.(githubProvider, { token: githubTokenDraft.trim() });
    setGithubTokenDraft("");
    setGithubTokenModalOpen(false);
  };

  const handleBifrostCloudUrlSave = (provider: SyncProviderStatus) => {
    onProviderRemoteBaseUrlSave?.(provider, bifrostCloudUrlDraft);
    setBifrostCloudUrlDirty(false);
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
          gridTemplateColumns: "repeat(auto-fit, minmax(min(100%, 380px), 1fr))",
          maxWidth: 1440,
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
                    children:
                      provider.id === "bifrost_cloud" ? (
                        <Space.Compact style={{ width: "100%" }}>
                          <Input
                            value={bifrostCloudUrlDraft}
                            onChange={(event) => {
                              setBifrostCloudUrlDirty(true);
                              setBifrostCloudUrlDraft(event.target.value);
                            }}
                            placeholder="https://sync.bifrostproxy.dev"
                            prefix={<LinkOutlined />}
                            data-testid="settings-sync-provider-bifrost-cloud-url-input"
                          />
                          <Button
                            type="primary"
                            onClick={() => handleBifrostCloudUrlSave(provider)}
                            loading={syncLoading}
                            data-testid="settings-sync-provider-bifrost-cloud-url-save"
                          >
                            Save
                          </Button>
                        </Space.Compact>
                      ) : (
                        provider.remote_base_url || "GitHub Gist"
                      ),
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
                  icon={provider.id === "github_gist" ? <GithubOutlined /> : <LoginOutlined />}
                  onClick={() => handleProviderSignIn(provider)}
                  disabled={syncLoading}
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

      <Modal
        title="Sign in to GitHub Gist"
        open={githubTokenModalOpen}
        onCancel={() => setGithubTokenModalOpen(false)}
        onOk={handleGithubGistSignIn}
        okText="Sign In"
        okButtonProps={{
          disabled: !githubTokenDraft.trim(),
          loading: syncLoading,
        }}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Text type="secondary">
            Enter a GitHub token with the gist scope. The token is stored locally and
            used only for your Gist-backed sync provider.
          </Text>
          <Input.Password
            value={githubTokenDraft}
            onChange={(event) => setGithubTokenDraft(event.target.value)}
            placeholder="ghp_..."
            prefix={<GithubOutlined />}
            autoComplete="off"
            data-testid="settings-sync-provider-github-gist-token-input"
          />
        </Space>
      </Modal>

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
            disabled={!selectedProvider}
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
