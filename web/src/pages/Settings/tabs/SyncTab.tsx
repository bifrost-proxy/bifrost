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
import {
  getDefaultRemoteBaseUrl,
  type SyncProviderStatus,
  type SyncStatus,
} from "../../../api/sync";
import {
  t as translateGlobal,
  useI18n,
  type TFunction,
  type TranslationKey,
} from "../../../i18n";

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

function capabilityTags(provider: SyncProviderStatus, t: TFunction) {
  return (
    <Space size={[4, 4]} wrap>
      <Tag color={provider.capabilities.remote_invoke ? "blue" : "default"}>
        {t("settings.tabRemoteInvoke")}
      </Tag>
      <Tag color={provider.capabilities.rules_sync ? "green" : "default"}>
        {t("settings.sync.rulesSync")}
      </Tag>
      <Tag color={provider.capabilities.config_sync ? "cyan" : "default"}>
        {t("settings.sync.configSync")}
      </Tag>
    </Space>
  );
}

export function syncProviderStatusBadge(
  provider: SyncProviderStatus,
  t: TFunction = translateGlobal,
) {
  if (provider.connected && !provider.authorized) {
    return { color: "red", label: t("settings.sync.badgeReconnect") };
  }
  if (provider.last_error && provider.reason === "error") {
    return { color: "red", label: t("settings.sync.badgeError") };
  }
  if (provider.connected) {
    return { color: "green", label: t("settings.sync.badgeConnected") };
  }
  if (provider.enabled && provider.reachable) {
    return { color: "gold", label: t("settings.sync.badgeSignIn") };
  }
  if (provider.enabled && !provider.reachable) {
    return { color: "orange", label: t("settings.sync.badgeOffline") };
  }
  return { color: undefined, label: t("settings.sync.badgeNotConnected") };
}

export function syncProviderActionableError(provider: SyncProviderStatus) {
  return provider.authorized && provider.reason === "ready"
    ? null
    : provider.last_error;
}

export function shouldShowSyncProviderOverviewAlert(
  providers: SyncProviderStatus[],
) {
  return !providers.some((provider) => provider.connected);
}

function formatSyncMoment(
  t: TFunction,
  syncAt?: string | null,
  syncAction?: SyncProviderStatus["last_sync_action"],
) {
  if (!syncAt) {
    return t("settings.sync.never");
  }
  const date = new Date(syncAt);
  const action = syncAction
    ? syncAction.replace(/_/g, " ")
    : t("settings.sync.completed");
  return `${date.toLocaleString()} (${action})`;
}

function formatProviderLastChange(provider: SyncProviderStatus, t: TFunction) {
  return formatSyncMoment(
    t,
    provider.last_changed_sync_at,
    provider.last_changed_sync_action,
  );
}

function formatProviderLastCheck(provider: SyncProviderStatus, t: TFunction) {
  return formatSyncMoment(t, provider.last_sync_at, provider.last_sync_action);
}

function formatProviderCheckInterval(
  provider: SyncProviderStatus,
  t: TFunction,
) {
  const seconds = provider.check_interval_secs;
  if (!seconds || seconds <= 0) {
    return t("settings.sync.manual");
  }
  if (seconds % 3600 === 0) {
    const hours = seconds / 3600;
    return hours === 1
      ? t("settings.sync.everyHourOne", { count: hours })
      : t("settings.sync.everyHourOther", { count: hours });
  }
  if (seconds % 60 === 0) {
    const minutes = seconds / 60;
    return t("settings.sync.everyMinutes", { count: minutes });
  }
  return t("settings.sync.everySeconds", { count: seconds });
}

function providerStatusTag(provider: SyncProviderStatus, t: TFunction) {
  const badge = syncProviderStatusBadge(provider, t);
  return <Tag color={badge.color}>{badge.label}</Tag>;
}

const getFallbackProviders = (t: TFunction): SyncProviderStatus[] => [
  {
    id: "bytedance_internal",
    name: "ByteDance Internal",
    description: t("settings.sync.fallbackBytedance"),
    remote_base_url: getDefaultRemoteBaseUrl(),
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
    last_changed_sync_at: null,
    last_changed_sync_action: null,
    check_interval_secs: 300,
    user: null,
    capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
  {
    id: "bifrost_cloud",
    name: "Bifrost Cloud",
    description: t("settings.sync.fallbackBifrostCloud"),
    remote_base_url: null,
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
    last_changed_sync_at: null,
    last_changed_sync_action: null,
    check_interval_secs: 300,
    user: null,
    capabilities: { remote_invoke: true, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
  {
    id: "github_gist",
    name: "GitHub Gist",
    description: t("settings.sync.fallbackGithubGist"),
    remote_base_url: null,
    connected: false,
    enabled: false,
    reachable: false,
    authorized: false,
    reason: "unauthorized",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
    last_changed_sync_at: null,
    last_changed_sync_action: null,
    check_interval_secs: 300,
    user: null,
    capabilities: { remote_invoke: false, rules_sync: true, config_sync: true },
    remote_invoke_registered: false,
  },
];

// Provider descriptions come from the backend in English; show the
// translated copy for the known built-in providers.
const PROVIDER_DESCRIPTION_KEYS: Record<string, TranslationKey> = {
  "Internal trusted sync and Remote Invoke provider.":
    "settings.sync.fallbackBytedance",
  "Custom Bifrost sync service for teams and self-hosting.":
    "settings.sync.fallbackBifrostCloud",
  "Public GitHub Gist-backed portable sync provider.":
    "settings.sync.fallbackGithubGist",
};

function localizeProviderDescription(
  description: string,
  t: TFunction,
): string {
  const key = PROVIDER_DESCRIPTION_KEYS[description];
  return key ? t(key) : description;
}

export default function SyncTab({
  syncStatus,
  syncLoading,
  onSignIn,
  onProviderSignIn,
  onProviderRemoteBaseUrlSave,
  onProviderSignOut,
  onSignOut,
}: SyncTabProps) {
  const { t } = useI18n();
  const fallbackProviders = useMemo(() => getFallbackProviders(t), [t]);
  const providers = syncStatus?.providers?.length
    ? syncStatus.providers
    : fallbackProviders;
  const [firstRunOpen, setFirstRunOpen] = useState(false);
  const [selectedProviderId, setSelectedProviderId] = useState(
    providers[0]?.id,
  );
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
    () =>
      providers.find((provider) => provider.id === selectedProviderId) ||
      providers[0],
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
      remote_base_url:
        bifrostCloudUrlDraft.trim() || provider.remote_base_url || null,
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
    const githubProvider = providers.find(
      (provider) => provider.id === "github_gist",
    );
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
          message={t("settings.sync.overviewTitle")}
          description={t("settings.sync.overviewDescription")}
          data-testid="settings-sync-provider-overview-alert"
        />
      ) : null}

      <div
        data-testid="settings-sync-provider-grid"
        style={{
          display: "grid",
          gap: 16,
          gridTemplateColumns:
            "repeat(auto-fit, minmax(min(100%, 380px), 1fr))",
          maxWidth: 1440,
          width: "100%",
        }}
      >
        {providers.map((provider) => {
          const actionableError = syncProviderActionableError(provider);
          return (
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
              extra={providerStatusTag(provider, t)}
            >
              <Space direction="vertical" size={12} style={{ width: "100%" }}>
                <Text type="secondary">
                  {localizeProviderDescription(provider.description, t)}
                </Text>
                {provider.id === "github_gist" ? (
                  <Alert
                    type={actionableError ? "error" : "info"}
                    showIcon
                    message={
                      actionableError
                        ? t("settings.sync.gistTokenAttention")
                        : t("settings.sync.gistTokenHint")
                    }
                    description={
                      actionableError || t("settings.sync.gistSnapshotHint")
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
                        {actionableError
                          ? t("settings.sync.newToken")
                          : t("settings.sync.generateToken")}
                      </Button>
                    }
                    data-testid={
                      actionableError
                        ? "settings-sync-provider-error-github_gist"
                        : "settings-sync-provider-github-gist-info"
                    }
                  />
                ) : null}
                {provider.id !== "github_gist" && actionableError ? (
                  <Alert
                    type="error"
                    showIcon
                    message={t("settings.sync.providerAttention", {
                      name: provider.name,
                    })}
                    description={actionableError}
                    data-testid={`settings-sync-provider-error-${provider.id}`}
                  />
                ) : null}
                {capabilityTags(provider, t)}
                <Descriptions
                  size="small"
                  column={1}
                  items={[
                    {
                      key: "account",
                      label: t("settings.sync.account"),
                      children:
                        provider.user?.user_id ||
                        (provider.connected
                          ? t("settings.sync.signedIn")
                          : t("settings.sync.notSignedIn")),
                    },
                    {
                      key: "remote",
                      label: t("settings.sync.remote"),
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
                              onClick={() =>
                                handleBifrostCloudUrlSave(provider)
                              }
                              loading={syncLoading}
                              data-testid="settings-sync-provider-bifrost-cloud-url-save"
                            >
                              {t("common.save")}
                            </Button>
                          </Space.Compact>
                        ) : (
                          provider.remote_base_url || "GitHub Gist"
                        ),
                    },
                    {
                      key: "invoke",
                      label: t("settings.tabRemoteInvoke"),
                      children: provider.capabilities.remote_invoke
                        ? provider.remote_invoke_registered
                          ? t("settings.sync.registered")
                          : t("settings.sync.supported")
                        : t("settings.sync.notSupported"),
                    },
                    {
                      key: "lastChange",
                      label: t("settings.sync.lastChange"),
                      children: formatProviderLastChange(provider, t),
                    },
                    {
                      key: "lastCheck",
                      label: t("settings.sync.lastCheck"),
                      children: formatProviderLastCheck(provider, t),
                    },
                    {
                      key: "checkInterval",
                      label: t("settings.sync.checkInterval"),
                      children: formatProviderCheckInterval(provider, t),
                    },
                  ]}
                />
                <Space wrap>
                  <Button
                    type={provider.connected ? "default" : "primary"}
                    icon={
                      provider.id === "github_gist" ? (
                        <GithubOutlined />
                      ) : (
                        <LoginOutlined />
                      )
                    }
                    onClick={() => handleProviderSignIn(provider)}
                    disabled={syncLoading}
                    loading={syncLoading}
                    data-testid={`settings-sync-provider-login-${provider.id}`}
                  >
                    {provider.connected
                      ? t("settings.sync.reconnect")
                      : t("settings.sync.signIn")}
                  </Button>
                  {provider.connected ? (
                    <Button
                      icon={<LogoutOutlined />}
                      onClick={() => handleProviderSignOut(provider)}
                      loading={syncLoading}
                      data-testid={`settings-sync-provider-logout-${provider.id}`}
                    >
                      {t("settings.sync.signOut")}
                    </Button>
                  ) : null}
                </Space>
              </Space>
            </Card>
          );
        })}
      </div>

      <Modal
        title={t("settings.sync.gistModalTitle")}
        open={githubTokenModalOpen}
        onCancel={() => setGithubTokenModalOpen(false)}
        onOk={handleGithubGistSignIn}
        okText={t("settings.sync.signIn")}
        okButtonProps={{
          disabled: !githubTokenDraft.trim(),
          loading: syncLoading,
        }}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert
            type="info"
            showIcon
            message={t("settings.sync.gistModalAlert")}
            action={
              <Button
                size="small"
                href={GITHUB_GIST_TOKEN_URL}
                target="_blank"
                rel="noreferrer"
                icon={<GithubOutlined />}
                data-testid="settings-sync-provider-github-gist-modal-token-link"
              >
                {t("settings.sync.generateToken")}
              </Button>
            }
          />
          <Text type="secondary">{t("settings.sync.gistModalHint")}</Text>
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
        title={t("settings.sync.firstRunTitle")}
        open={firstRunOpen}
        onCancel={() => setFirstRunOpen(false)}
        footer={[
          <Button key="cancel" onClick={() => setFirstRunOpen(false)}>
            {t("settings.sync.notNow")}
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
            {t("common.start")}
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
                  {capabilityTags(provider, t)}
                </Space>
              </Radio>
            ))}
          </Space>
        </Radio.Group>
      </Modal>
    </Space>
  );
}
