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
const GITHUB_GIST_TOKEN_URL =
  "https://github.com/settings/tokens/new?description=Bifrost%20Sync&scopes=gist";
const BIFROST_CLOUD_URL_PLACEHOLDER = "https://your-sync.example.com";

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
  onProviderSignOut?: (provider: SyncProviderStatus) => void;
  onSignOut?: () => void;
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

export function syncProviderStatusBadge(provider: SyncProviderStatus) {
  if (provider.last_error || (provider.connected && !provider.authorized)) {
    return { color: "red", label: "Reconnect required" };
  }
  if (provider.connected) {
    return { color: "green", label: "Connected" };
  }
  if (provider.enabled && provider.reachable) {
    return { color: "gold", label: "Sign in required" };
  }
  if (provider.enabled && !provider.reachable) {
    return { color: "orange", label: "Offline" };
  }
  return { color: undefined, label: "Not connected" };
}

export function shouldShowSyncProviderOverviewAlert(providers: SyncProviderStatus[]) {
  return !providers.some((provider) => provider.connected);
}

function formatProviderLastSync(provider: SyncProviderStatus) {
  if (!provider.last_sync_at) {
    return "Never";
  }
  const date = new Date(provider.last_sync_at);
  const action = provider.last_sync_action
    ? provider.last_sync_action.replace(/_/g, " ")
    : "completed";
  return `${date.toLocaleString()} (${action})`;
}

function providerStatusTag(provider: SyncProviderStatus) {
  const badge = syncProviderStatusBadge(provider);
  return <Tag color={badge.color}>{badge.label}</Tag>;
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
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
    user: null,
    capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
  {
    id: "bifrost_cloud",
    name: "Bifrost Cloud",
    description: "Custom Bifrost sync service for teams and self-hosting.",
    remote_base_url: null,
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
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
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
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
  onProviderSignOut,
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
    bifrostCloudProvider?.remote_base_url || "",
  );
  const [bifrostCloudUrlDirty, setBifrostCloudUrlDirty] = useState(false);
  const [githubTokenModalOpen, setGithubTokenModalOpen] = useState(false);
  const [githubTokenDraft, setGithubTokenDraft] = useState("");
  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === selectedProviderId) || providers[0],
    [providers, selectedProviderId],
  );
  const showOverviewAlert = shouldShowSyncProviderOverviewAlert(providers);

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
    setBifrostCloudUrlDraft(bifrostCloudProvider?.remote_base_url || "");
  }, [bifrostCloudProvider?.remote_base_url, bifrostCloudUrlDirty]);

  const providerWithDraftUrl = (provider: SyncProviderStatus) => {
    if (provider.id !== "bifrost_cloud") {
      return provider;
    }
    return {
      ...provider,
      remote_base_url: bifrostCloudUrlDraft.trim() || provider.remote_base_url || null,
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

  const handleProviderSignOut = (provider: SyncProviderStatus) => {
    if (onProviderSignOut) {
      onProviderSignOut(provider);
      return;
    }
    onSignOut?.();
  };

  return (
    <Space direction="vertical" size={16} style={{ width: "100%" }}>
      {showOverviewAlert ? (
        <Alert
          showIcon
          type={syncStatus?.reachable ? "info" : "warning"}
          message="Remote sync is a pluggable capability"
          description="Rules continue to work locally at all times. Sync only activates when the configured Bifrost service is reachable and a valid login session exists."
          data-testid="settings-sync-provider-overview-alert"
        />
      ) : null}

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
              {provider.id === "github_gist" ? (
                <Alert
                  type={provider.last_error ? "error" : "info"}
                  showIcon
                  message={
                    provider.last_error
                      ? "GitHub Gist token needs attention"
                      : "Generate a GitHub token with the gist scope, then paste it into Bifrost."
                  }
                  description={
                    provider.last_error ||
                    "Rules and basic settings are stored in a private Gist snapshot. Do not sync secrets here."
                  }
                  action={
                    <Button
                      size="small"
                      href={GITHUB_GIST_TOKEN_URL}
                      target="_blank"
                      rel="noreferrer"
                      icon={<GithubOutlined />}
                      data-testid="settings-sync-provider-github-gist-token-link"
                    >
                      {provider.last_error ? "New Token" : "Generate Token"}
                    </Button>
                  }
                  data-testid={
                    provider.last_error
                      ? "settings-sync-provider-error-github_gist"
                      : "settings-sync-provider-github-gist-info"
                  }
                />
              ) : null}
              {provider.id !== "github_gist" && provider.last_error ? (
                <Alert
                  type="error"
                  showIcon
                  message={`${provider.name} needs attention`}
                  description={provider.last_error}
                  data-testid={`settings-sync-provider-error-${provider.id}`}
                />
              ) : null}
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
                            placeholder={BIFROST_CLOUD_URL_PLACEHOLDER}
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
                  {
                    key: "lastSync",
                    label: "Last sync",
                    children: formatProviderLastSync(provider),
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
                    onClick={() => handleProviderSignOut(provider)}
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
          <Alert
            type="info"
            showIcon
            message="Create a GitHub token with only the gist scope selected."
            action={
              <Button
                size="small"
                href={GITHUB_GIST_TOKEN_URL}
                target="_blank"
                rel="noreferrer"
                icon={<GithubOutlined />}
                data-testid="settings-sync-provider-github-gist-modal-token-link"
              >
                Generate Token
              </Button>
            }
          />
          <Text type="secondary">
            Enter a GitHub token with the gist scope. The token is stored locally and
            used only for your Gist-backed sync provider. Bifrost stores rules and
            basic settings in a private Gist snapshot, so avoid syncing secrets.
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
