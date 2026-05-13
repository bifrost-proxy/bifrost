import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Button,
  Card,
  Checkbox,
  Col,
  Divider,
  Empty,
  Input,
  InputNumber,
  List,
  Row,
  Select,
  Space,
  Switch,
  Tag,
  Typography,
  message,
  theme,
} from "antd";
import { FileSearchOutlined, PlayCircleOutlined, ReloadOutlined } from "@ant-design/icons";
import { get, normalizeApiErrorMessage, patch, post } from "../../../../api/client";
import { buildApiUrl } from "../../../../runtime";
import { getAdminToken } from "../../../../services/adminAuth";
import { getClientId } from "../../../../services/clientId";
import { BASE, type ResearchConfig, type ResearchProviderConfig } from "./types";

const { Link, Text } = Typography;

interface ResearchSectionProps {
  value?: ResearchConfig;
  onChange: (value: ResearchConfig) => void;
}

interface KnowledgeResult {
  title: string;
  url: string;
  summary?: string;
  matched_text?: string;
}

interface ReportInfo {
  task_id: string;
  date: string;
  path: string;
}

interface ResearchSearchResult {
  id: string;
  source: "web" | "wechat";
  provider: string;
  title: string;
  url: string;
  canonical_url?: string;
  snippet?: string;
  site_name?: string;
  author?: string;
  published_at?: string;
  score?: number;
  content_hash?: string;
  content_markdown?: string;
  retrieved_at?: number;
}

interface ResearchSearchStreamEvent {
  type: "provider_result" | "done";
  provider_id?: string;
  results?: ResearchSearchResult[];
  error?: string;
}

interface ResearchCapability {
  id: string;
  label: string;
  source: "web" | "wechat";
  supported: boolean;
  configured: boolean;
  enabled: boolean;
  type: string;
  site?: string;
  authorized: boolean;
  authorization_status: string;
  logged_in: boolean;
  login_status: string;
  search_url_template?: string;
}

const edgeUserDataDirPlaceholder = "~/.bifrost/web/edge-user-data";

const defaultProviderOrder = [
  "volc_web_search",
  "sogou_wechat_cdp",
  "arxiv",
  "hacker_news",
  "github_repositories",
  "generic_web_search",
  "tavily",
  "exa",
  "custom_http",
  "mcp",
];

const defaultBuiltInProviders = (): Record<string, ResearchProviderConfig> => ({
  volc_web_search: {
    enabled: false,
    type: "volc_web_search",
    base_url: "https://open.feedcoopapi.com/search_api/web_search",
    env_key: "ARK_TOKEN",
    search_type: "web",
    count: 10,
    need_content: true,
    need_url: true,
    need_summary: false,
    content_formats: "markdown",
    query_rewrite: false,
  },
  sogou_wechat_cdp: {
    enabled: true,
    type: "sogou_wechat_cdp",
    cdp_endpoint: "http://127.0.0.1:9222",
    browser_user_data_dir: edgeUserDataDirPlaceholder,
  },
  arxiv: {
    enabled: true,
    type: "fixed_site",
    site: "arxiv",
  },
  hacker_news: {
    enabled: true,
    type: "fixed_site",
    site: "hacker_news",
  },
  github_repositories: {
    enabled: true,
    type: "fixed_site",
    site: "github_repositories",
  },
  generic_web_search: {
    enabled: false,
    type: "generic_web_search",
    base_url: "",
  },
  tavily: {
    enabled: false,
    type: "tavily",
    base_url: "",
    env_key: "TAVILY_API_KEY",
  },
  exa: {
    enabled: false,
    type: "exa",
    base_url: "",
    env_key: "EXA_API_KEY",
  },
  custom_http: {
    enabled: false,
    type: "custom_http",
    base_url: "",
  },
  mcp: {
    enabled: false,
    type: "mcp",
  },
});

const defaultResearch = (): ResearchConfig => ({
  enabled: false,
  preset: "personal-cn",
  providers: defaultBuiltInProviders(),
  provider_order: defaultProviderOrder,
  cache: { enabled: true, store: "sqlite", retention_days: 180 },
  defaults: {
    sources: ["web"],
    limit: 10,
    fetch_content: true,
    language: "zh-CN",
  },
  fetch_policy: {
    allow_private_ip: false,
    allow_localhost: false,
    max_redirects: 5,
    max_response_bytes: 500000,
    timeout_secs: 20,
    user_agent: "BifrostResearch/0.1",
  },
  tasks: [],
});

const providerLabels: Record<string, string> = {
  volc_web_search: "Volc",
  sogou_wechat_cdp: "Sogou WeChat",
  arxiv: "arXiv",
  hacker_news: "Hacker News",
  github_repositories: "GitHub Repos",
  generic_web_search: "Generic Web",
  tavily: "Tavily",
  exa: "Exa",
  custom_http: "Custom HTTP",
  mcp: "MCP",
};

const providerSource = (provider: ResearchProviderConfig): "web" | "wechat" =>
  provider.type === "sogou_wechat_cdp" ? "wechat" : "web";

const providerNeedsSecret = (provider: ResearchProviderConfig) =>
  provider.type === "volc_web_search" || provider.type === "tavily" || provider.type === "exa";

const providerNeedsEndpoint = (provider: ResearchProviderConfig) =>
  provider.type === "generic_web_search" || provider.type === "custom_http";

const providerIsReserved = (provider: ResearchProviderConfig) => provider.type === "mcp";

const providerCanSearch = (provider: ResearchProviderConfig) => provider.enabled && !providerIsReserved(provider);

const providerCredentialGuide = (provider: ResearchProviderConfig) => {
  if (provider.type === "volc_web_search") {
    return {
      getKeyLabel: "Create Volc Web Search API key",
      getKeyUrl: "https://console.volcengine.com/search-infinity/api-key",
      setupLabel: "Open Web Search console",
      setupUrl: "https://console.volcengine.com/search-infinity/web-search",
      envKey: provider.env_key || "ARK_TOKEN",
      note: "Use the Volc Web Search API key. A general model key or endpoint id is not enough.",
    };
  }
  if (provider.type === "tavily") {
    return {
      getKeyLabel: "Get Tavily API key",
      getKeyUrl: "https://app.tavily.com/",
      setupLabel: "Tavily quickstart",
      setupUrl: "https://docs.tavily.com/documentation/quickstart",
      envKey: provider.env_key || "TAVILY_API_KEY",
      note: "Sign in to Tavily Platform, copy an API key from the dashboard, then set the env var.",
    };
  }
  if (provider.type === "exa") {
    return {
      getKeyLabel: "Create Exa API key",
      getKeyUrl: "https://dashboard.exa.ai/api-keys",
      setupLabel: "Exa quickstart",
      setupUrl: "https://exa.ai/docs/reference/quickstart",
      envKey: provider.env_key || "EXA_API_KEY",
      note: "Create a key in the Exa Dashboard, then set the env var before starting Bifrost.",
    };
  }
  return undefined;
};

const renderProviderCredentialGuide = (provider: ResearchProviderConfig) => {
  const guide = providerCredentialGuide(provider);
  if (!guide) {
    return null;
  }
  return (
    <Row gutter={[8, 8]} align="top">
      <Col flex="120px" />
      <Col flex="auto">
        <Space direction="vertical" size={2} style={{ width: "100%" }}>
          <Space wrap size={8}>
            <Text type="secondary">Get key:</Text>
            <Link href={guide.getKeyUrl} target="_blank" rel="noreferrer">
              {guide.getKeyLabel}
            </Link>
            <Link href={guide.setupUrl} target="_blank" rel="noreferrer">
              {guide.setupLabel}
            </Link>
          </Space>
          <Text type="secondary">
            Preferred setup: set <Text code>{guide.envKey}</Text> before starting Bifrost, for example{" "}
            <Text code>{`export ${guide.envKey}=...`}</Text>. Direct API key is only for local testing.
          </Text>
          <Text type="secondary">{guide.note}</Text>
        </Space>
      </Col>
    </Row>
  );
};

const mergeResearchDefaults = (value?: ResearchConfig): ResearchConfig => {
  const base = defaultResearch();
  const baseProviderOrder = base.provider_order || [];
  const valueProviderOrder = value?.provider_order || [];
  const baseCache = base.cache || { enabled: true, store: "sqlite", retention_days: 180 };
  const providers = {
    ...base.providers,
    ...(value?.providers || {}),
  };
  const provider_order = [
    ...valueProviderOrder,
    ...baseProviderOrder.filter((id) => !valueProviderOrder.includes(id)),
  ];
  return {
    ...base,
    ...value,
    providers,
    provider_order,
    cache: { ...baseCache, ...(value?.cache || {}) },
    defaults: { ...base.defaults, ...(value?.defaults || {}) },
    fetch_policy: { ...base.fetch_policy, ...(value?.fetch_policy || {}) },
  };
};

export default function ResearchSection({ value, onChange }: ResearchSectionProps) {
  const { token } = theme.useToken();
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState("");
  const [liveQuery, setLiveQuery] = useState("");
  const [liveLimit, setLiveLimit] = useState(10);
  const [liveProviderIds, setLiveProviderIds] = useState<string[]>([
    "arxiv",
    "hacker_news",
    "github_repositories",
  ]);
  const [fetchContent, setFetchContent] = useState(true);
  const [liveControlsDirty, setLiveControlsDirty] = useState(false);
  const [results, setResults] = useState<KnowledgeResult[]>([]);
  const [searchResults, setSearchResults] = useState<ResearchSearchResult[]>([]);
  const [capabilities, setCapabilities] = useState<ResearchCapability[]>([]);
  const [reports, setReports] = useState<ReportInfo[]>([]);
  const research = useMemo(() => mergeResearchDefaults(value), [value]);

  const loadProviderStatus = useCallback(async () => {
    try {
      const data = await get<{ capabilities: ResearchCapability[] }>(
        `${BASE}/agent/research/capabilities`,
      );
      setCapabilities(data.capabilities || []);
    } catch {
      setCapabilities([]);
    }
  }, []);

  const saveResearch = useCallback(
    async (next: ResearchConfig, options?: { silent?: boolean }) => {
      setSaving(true);
      try {
        const updated = await patch<ResearchConfig>(`${BASE}/agent/research/config`, next);
        onChange(updated ?? next);
        if (!options?.silent) {
          message.success("Updated research settings");
        }
        loadProviderStatus();
      } catch {
        if (!options?.silent) {
          message.error("Failed to update research settings");
        }
        if (options?.silent) {
          throw new Error("Failed to update research settings");
        }
      } finally {
        setSaving(false);
      }
    },
    [loadProviderStatus, onChange],
  );

  const loadReports = useCallback(async () => {
    try {
      const data = await get<{ reports: ReportInfo[] }>(`${BASE}/agent/research/reports`);
      setReports(data.reports || []);
    } catch {
      setReports([]);
    }
  }, []);

  useEffect(() => {
    loadReports();
    loadProviderStatus();
  }, [loadReports, loadProviderStatus]);

  useEffect(() => {
    if (liveControlsDirty) {
      return;
    }
    setLiveLimit(research.defaults?.limit || 10);
    setFetchContent(research.defaults?.fetch_content ?? true);
  }, [
    liveControlsDirty,
    research.defaults?.fetch_content,
    research.defaults?.limit,
  ]);

  const runResearchSearch = async () => {
    if (!liveQuery.trim()) return;
    const selectedProviderIds = liveProviderIds.filter((id) => {
      const provider = research.providers?.[id];
      return provider && providerCanSearch(provider);
    });
    if (selectedProviderIds.length === 0) {
      message.warning("Select at least one enabled provider");
      return;
    }
    const selectedSources = Array.from(
      new Set(selectedProviderIds.map((id) => providerSource(research.providers[id]))),
    );
    setSearching(true);
    try {
      const activeResearch = {
        ...research,
        enabled: true,
        defaults: {
          ...research.defaults,
          sources: selectedSources,
          limit: liveLimit,
          fetch_content: fetchContent,
          language: research.defaults?.language || "zh-CN",
        },
      };
      onChange(activeResearch);
      await saveResearch(activeResearch, { silent: true });
      setLiveControlsDirty(false);
      setSearchResults([]);
      const headers: Record<string, string> = {
        "Content-Type": "application/json",
        "X-Client-Id": getClientId(),
      };
      const token = getAdminToken();
      if (token) headers.Authorization = `Bearer ${token}`;
      const response = await fetch(buildApiUrl(`${BASE}/agent/research/search/stream`), {
        method: "POST",
        headers,
        body: JSON.stringify({
          query: liveQuery.trim(),
          sources: selectedSources,
          provider_ids: selectedProviderIds,
          freshness: null,
          limit: liveLimit,
          fetch_content: fetchContent,
          language: research.defaults?.language || "zh-CN",
        }),
      });
      if (!response.ok) {
        const payload = await response.json().catch(() => null);
        throw new Error(payload?.error || `Research search failed (${response.status})`);
      }
      if (!response.body) {
        throw new Error("Research search stream is unavailable");
      }
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      let resultCount = 0;
      const providerErrors: string[] = [];
      const handleLine = (line: string) => {
        if (!line.trim()) return;
        const event = JSON.parse(line) as ResearchSearchStreamEvent;
        if (event.type !== "provider_result") return;
        if (event.error && event.provider_id) {
          providerErrors.push(`${event.provider_id}: ${event.error}`);
        }
        const nextResults = event.results || [];
        if (nextResults.length) {
          resultCount += nextResults.length;
          setSearchResults((prev) => [...prev, ...nextResults]);
        }
      };
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() || "";
        lines.forEach(handleLine);
      }
      buffer += decoder.decode();
      handleLine(buffer);
      if (resultCount === 0 && providerErrors.length > 0) {
        throw new Error(providerErrors.join("; "));
      }
      if (resultCount === 0) {
        message.info("No research results returned");
      } else if (providerErrors.length > 0) {
        message.warning(`Some providers failed: ${providerErrors.join("; ")}`);
      }
    } catch (error) {
      message.error(normalizeApiErrorMessage(error, "Research search failed"));
    } finally {
      setSearching(false);
    }
  };

  const searchKnowledge = async () => {
    if (!query.trim()) return;
    try {
      const data = await get<{ results: KnowledgeResult[] }>(
        `${BASE}/agent/research/items?query=${encodeURIComponent(query)}&limit=10`,
      );
      setResults(data.results || []);
    } catch {
      message.error("Knowledge search failed");
    }
  };

  const testProvider = async (providerId?: string) => {
    setTesting(true);
    try {
      await post(`${BASE}/agent/research/providers/test`, {
        provider_id: providerId,
        query: liveQuery.trim() || "语音大模型",
        limit: 3,
      });
      message.success(providerId ? `${providerId} test completed` : "Provider test completed");
    } catch (error) {
      const detail = normalizeApiErrorMessage(error);
      message.error(providerId ? `${providerId} test failed: ${detail}` : `Provider test failed: ${detail}`);
    } finally {
      setTesting(false);
    }
  };

  const providerEntries = Object.entries(research.providers || {});
  const providerOptions = providerEntries.map(([id, provider]) => ({
    label: (
      <Space size={4}>
        <span>{providerLabels[id] || id}</span>
        <Tag>{providerSource(provider)}</Tag>
        <Tag>{provider.type}</Tag>
      </Space>
    ),
    value: id,
    disabled: !providerCanSearch(provider),
  }));

  const updateProvider = (id: string, provider: ResearchProviderConfig) => {
    onChange({
      ...research,
      providers: {
        ...(research.providers || {}),
        [id]: provider,
      },
      provider_order: research.provider_order?.includes(id)
        ? research.provider_order
        : [...(research.provider_order || []), id],
    });
  };

  const saveProvider = (id: string, provider: ResearchProviderConfig) => {
    saveResearch({
      ...research,
      providers: {
        ...(research.providers || {}),
        [id]: provider,
      },
      provider_order: research.provider_order?.includes(id)
        ? research.provider_order
        : [...(research.provider_order || []), id],
    });
  };

  const capabilityTag = (capability: ResearchCapability) => {
    const authText =
      capability.authorization_status === "reserved"
        ? "Reserved"
        : !capability.supported
          ? "Unsupported"
          : capability.authorization_status === "not_required"
            ? "Auth not required"
            : capability.authorized
              ? "Authorized"
              : "Auth missing";
    const supportedText = capability.supported ? "Supported" : "Reserved";
    const supportedColor = capability.supported ? "blue" : "default";
    const loginText =
      !capability.supported
        ? "Not available"
        : capability.login_status === "not_required"
          ? "Login not required"
          : capability.logged_in
            ? "Logged in"
            : capability.login_status === "browser_not_connected"
              ? "Browser offline"
              : "Not logged in";
    return (
      <Space wrap size={4}>
        <Tag color={supportedColor}>{supportedText}</Tag>
        <Tag color={capability.enabled ? "green" : capability.configured ? "gold" : "default"}>
          {capability.enabled ? "Enabled" : capability.configured ? "Configured" : "Not configured"}
        </Tag>
        <Tag color={capability.authorized ? "green" : capability.supported ? "red" : "default"}>{authText}</Tag>
        <Tag
          color={
            capability.logged_in ? "green" : capability.login_status === "not_required" ? "blue" : "orange"
          }
        >
          {loginText}
        </Tag>
      </Space>
    );
  };

  return (
    <Space direction="vertical" style={{ width: "100%" }} size={16}>
      <Card
        size="small"
        title={
          <Space>
            <FileSearchOutlined />
            <span>Research Pack</span>
          </Space>
        }
        extra={
          <Tag color={research.enabled ? "green" : "default"}>
            {research.enabled ? "Enabled" : "Disabled"}
          </Tag>
        }
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Row justify="space-between" align="middle">
            <Col>
              <Text>Enable Research Pack</Text>
            </Col>
            <Col>
              <Switch
                checked={research.enabled}
                loading={saving}
                onChange={(enabled) => saveResearch({ ...research, enabled })}
              />
            </Col>
          </Row>
          <Divider style={{ margin: "12px 0" }} />
          <Row gutter={12} align="middle">
            <Col flex="160px">
              <Text>Preset</Text>
            </Col>
            <Col flex="auto">
              <Input
                value={research.preset}
                onChange={(event) => onChange({ ...research, preset: event.target.value })}
                onBlur={() => saveResearch(research)}
                placeholder="personal-cn"
                size="small"
              />
            </Col>
          </Row>
        </Space>
      </Card>

      <Card size="small" title="Research Search">
        <Space direction="vertical" style={{ width: "100%" }} size={12}>
          <Row gutter={12} align="middle">
            <Col flex="auto">
              <Input
                value={liveQuery}
                onChange={(event) => setLiveQuery(event.target.value)}
                onPressEnter={runResearchSearch}
                placeholder="Enter a keyword, for example AI HUB"
                size="small"
              />
            </Col>
            <Col>
              <InputNumber
                size="small"
                min={1}
                max={20}
                value={liveLimit}
                onChange={(value) => {
                  setLiveControlsDirty(true);
                  setLiveLimit(value || 10);
                }}
              />
            </Col>
            <Col>
              <Button size="small" type="primary" loading={searching} onClick={runResearchSearch}>
                Search
              </Button>
            </Col>
          </Row>
          <Row gutter={12} align="middle">
            <Col>
              <Checkbox.Group
                value={liveProviderIds}
                options={providerOptions}
                onChange={(checked) => {
                  setLiveControlsDirty(true);
                  setLiveProviderIds(checked as string[]);
                }}
              />
            </Col>
            <Col>
              <Switch
                checked={fetchContent}
                size="small"
                onChange={(checked) => {
                  setLiveControlsDirty(true);
                  setFetchContent(checked);
                }}
              />
            </Col>
            <Col>
              <Text>Fetch full Markdown</Text>
            </Col>
          </Row>
          {searchResults.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No live research results yet" />
          ) : (
            <List
              size="small"
              dataSource={searchResults}
              renderItem={(item) => (
                <List.Item>
                  <Space direction="vertical" style={{ width: "100%" }} size={6}>
                    <Space wrap>
                      <a href={item.url} target="_blank" rel="noreferrer">
                        <Text strong>{item.title}</Text>
                      </a>
                      <Tag>{item.source}</Tag>
                      <Tag>{item.provider}</Tag>
                      {item.site_name && <Tag>{item.site_name}</Tag>}
                    </Space>
                    <Space wrap size={8}>
                      {item.author && <Text type="secondary">author: {item.author}</Text>}
                      {item.published_at && <Text type="secondary">published: {item.published_at}</Text>}
                      {item.retrieved_at && <Text type="secondary">retrieved: {item.retrieved_at}</Text>}
                      {item.content_hash && <Text type="secondary">hash: {item.content_hash.slice(0, 12)}</Text>}
                    </Space>
                    {item.snippet && <Text type="secondary">{item.snippet}</Text>}
                    {item.canonical_url && (
                      <Text type="secondary" copyable style={{ wordBreak: "break-all" }}>
                        {item.canonical_url}
                      </Text>
                    )}
                    {item.content_markdown && (
                      <pre
                        style={{
                          margin: 0,
                          padding: 12,
                          maxHeight: 260,
                          overflow: "auto",
                          whiteSpace: "pre-wrap",
                          borderRadius: token.borderRadiusSM,
                          background: token.colorFillTertiary,
                          color: token.colorText,
                          border: `1px solid ${token.colorBorderSecondary}`,
                        }}
                      >
                        {item.content_markdown}
                      </pre>
                    )}
                  </Space>
                </List.Item>
              )}
            />
          )}
        </Space>
      </Card>

      <Card
        size="small"
        title="Supported Sources"
        extra={
          <Button size="small" icon={<ReloadOutlined />} onClick={loadProviderStatus}>
            Refresh
          </Button>
        }
      >
        {capabilities.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No research sources reported" />
        ) : (
          <List
            size="small"
            dataSource={capabilities}
            renderItem={(capability) => (
              <List.Item>
                <List.Item.Meta
                  title={
                    <Space wrap>
                      <Text strong>{capability.label}</Text>
                      <Tag>{capability.source}</Tag>
                      <Tag>{capability.type}</Tag>
                      {capabilityTag(capability)}
                    </Space>
                  }
                  description={
                    <Space direction="vertical" size={2} style={{ width: "100%" }}>
                      {capability.search_url_template && (
                        <Text type="secondary" copyable style={{ wordBreak: "break-all" }}>
                          {capability.search_url_template}
                        </Text>
                      )}
                    </Space>
                  }
                />
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card
        size="small"
        title="Providers"
        extra={
          <Button
            size="small"
            icon={<PlayCircleOutlined />}
            loading={testing}
            onClick={() => testProvider()}
          >
            Test
          </Button>
        }
      >
        {providerEntries.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No research providers configured" />
        ) : (
          <List
            dataSource={providerEntries}
            renderItem={([id, provider]) => (
              <List.Item>
                <List.Item.Meta
                  title={
                    <Space wrap>
                      <Text strong>{id}</Text>
                      <Select
                        size="small"
                        value={provider.type}
                        style={{ width: 180 }}
                        onChange={(type) => saveProvider(id, { ...provider, type })}
                        options={[
                          { label: "Volc Web Search", value: "volc_web_search" },
                          { label: "Generic Web Search", value: "generic_web_search" },
                          { label: "Sogou WeChat CDP", value: "sogou_wechat_cdp" },
                          { label: "Fixed Site", value: "fixed_site" },
                          { label: "Tavily", value: "tavily" },
                          { label: "Exa", value: "exa" },
                          { label: "Custom HTTP", value: "custom_http" },
                          { label: "MCP", value: "mcp" },
                        ]}
                      />
                      <Tag color={provider.enabled ? "green" : "default"}>
                        {provider.enabled ? "enabled" : "disabled"}
                      </Tag>
                      <Switch
                        size="small"
                        checked={provider.enabled}
                        loading={saving}
                        onChange={(enabled) => saveProvider(id, { ...provider, enabled })}
                      />
                      {!providerIsReserved(provider) && (
                        <Button
                          size="small"
                          icon={<PlayCircleOutlined />}
                          loading={testing}
                          onClick={() => testProvider(id)}
                        >
                          Test
                        </Button>
                      )}
                    </Space>
                  }
                  description={
                    <Space direction="vertical" size={8} style={{ width: "100%" }}>
                      {provider.type === "volc_web_search" && (
                        <Text type="secondary" copyable>
                          https://open.feedcoopapi.com/search_api/web_search
                        </Text>
                      )}
                      {providerNeedsEndpoint(provider) && (
                        <Row gutter={[8, 8]} align="middle">
                          <Col flex="120px">
                            <Text type="secondary">Endpoint</Text>
                          </Col>
                          <Col flex="auto">
                            <Input
                              size="small"
                              value={provider.base_url || ""}
                              placeholder="https://your-search-provider.example/search"
                              onChange={(event) => updateProvider(id, { ...provider, base_url: event.target.value })}
                              onBlur={(event) => saveProvider(id, { ...provider, base_url: event.target.value })}
                            />
                          </Col>
                        </Row>
                      )}
                      {providerNeedsSecret(provider) && (
                        <>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px">
                              <Text type="secondary">Secret</Text>
                            </Col>
                            <Col flex="180px">
                              <Input
                                size="small"
                                value={provider.env_key || ""}
                                placeholder={
                                  provider.type === "tavily"
                                    ? "TAVILY_API_KEY"
                                    : provider.type === "exa"
                                      ? "EXA_API_KEY"
                                      : "ARK_TOKEN"
                                }
                                addonBefore="$"
                                onChange={(event) => updateProvider(id, { ...provider, env_key: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, env_key: event.target.value })}
                              />
                            </Col>
                            <Col flex="auto">
                              <Input.Password
                                size="small"
                                value={provider.api_key || ""}
                                placeholder="Optional direct API key"
                                onChange={(event) => updateProvider(id, { ...provider, api_key: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, api_key: event.target.value })}
                              />
                            </Col>
                            {(provider.api_key || provider.env_key) && (
                              <Col>
                                <Tag color="blue">{provider.env_key ? `$${provider.env_key}` : "configured"}</Tag>
                              </Col>
                            )}
                          </Row>
                          {renderProviderCredentialGuide(provider)}
                        </>
                      )}
                      {provider.type === "fixed_site" && (
                        <Text type="secondary">
                          Built-in site source: {provider.site || id}. No endpoint or token is required.
                        </Text>
                      )}
                      {provider.type === "sogou_wechat_cdp" && (
                        <>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px">
                              <Text type="secondary">CDP Endpoint</Text>
                            </Col>
                            <Col flex="auto">
                              <Input
                                size="small"
                                value={provider.cdp_endpoint || ""}
                                placeholder="http://127.0.0.1:9222"
                                onChange={(event) => updateProvider(id, { ...provider, cdp_endpoint: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, cdp_endpoint: event.target.value })}
                              />
                            </Col>
                          </Row>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px">
                              <Text type="secondary">Browser Data</Text>
                            </Col>
                            <Col flex="auto">
                              <Input
                                size="small"
                                value={provider.browser_user_data_dir || ""}
                                placeholder={edgeUserDataDirPlaceholder}
                                onChange={(event) =>
                                  updateProvider(id, { ...provider, browser_user_data_dir: event.target.value })
                                }
                                onBlur={(event) =>
                                  saveProvider(id, { ...provider, browser_user_data_dir: event.target.value })
                                }
                              />
                            </Col>
                          </Row>
                        </>
                      )}
                      {provider.type === "mcp" && (
                        <Text type="secondary">
                          MCP research provider is reserved for the MCP-backed source bridge.
                        </Text>
                      )}
                      {provider.type === "volc_web_search" && (
                        <>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px">
                              <Text type="secondary">Search</Text>
                            </Col>
                            <Col flex="150px">
                              <Select
                                size="small"
                                value={provider.search_type || "web"}
                                style={{ width: "100%" }}
                                onChange={(search_type) => saveProvider(id, { ...provider, search_type })}
                                options={[
                                  { label: "web", value: "web" },
                                  { label: "web_summary", value: "web_summary" },
                                  { label: "image", value: "image" },
                                ]}
                              />
                            </Col>
                            <Col flex="120px">
                              <InputNumber
                                size="small"
                                min={1}
                                max={(provider.search_type || "web") === "image" ? 5 : 50}
                                value={provider.count || 10}
                                onChange={(count) => saveProvider(id, { ...provider, count: count || 10 })}
                              />
                            </Col>
                            <Col>
                              <Checkbox
                                checked={provider.need_content ?? true}
                                onChange={(event) => saveProvider(id, { ...provider, need_content: event.target.checked })}
                              >
                                Content
                              </Checkbox>
                            </Col>
                            <Col>
                              <Checkbox
                                checked={provider.need_url ?? true}
                                onChange={(event) => saveProvider(id, { ...provider, need_url: event.target.checked })}
                              >
                                URL
                              </Checkbox>
                            </Col>
                            <Col>
                              <Checkbox
                                checked={provider.need_summary ?? provider.search_type === "web_summary"}
                                onChange={(event) => saveProvider(id, { ...provider, need_summary: event.target.checked })}
                              >
                                Summary
                              </Checkbox>
                            </Col>
                            <Col>
                              <Checkbox
                                checked={provider.query_rewrite ?? false}
                                onChange={(event) => saveProvider(id, { ...provider, query_rewrite: event.target.checked })}
                              >
                                Rewrite
                              </Checkbox>
                            </Col>
                          </Row>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px">
                              <Text type="secondary">Filters</Text>
                            </Col>
                            <Col flex="130px">
                              <Select
                                size="small"
                                value={provider.content_formats || "markdown"}
                                style={{ width: "100%" }}
                                onChange={(content_formats) => saveProvider(id, { ...provider, content_formats })}
                                options={[
                                  { label: "markdown", value: "markdown" },
                                  { label: "text", value: "text" },
                                ]}
                              />
                            </Col>
                            <Col flex="150px">
                              <Input
                                size="small"
                                value={provider.time_range || ""}
                                placeholder="OneWeek"
                                onChange={(event) => updateProvider(id, { ...provider, time_range: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, time_range: event.target.value })}
                              />
                            </Col>
                            <Col flex="auto">
                              <Input
                                size="small"
                                value={provider.sites || ""}
                                placeholder="Sites, e.g. mp.qq.com|volcengine.com"
                                onChange={(event) => updateProvider(id, { ...provider, sites: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, sites: event.target.value })}
                              />
                            </Col>
                          </Row>
                          <Row gutter={[8, 8]} align="middle">
                            <Col flex="120px" />
                            <Col flex="auto">
                              <Input
                                size="small"
                                value={provider.block_hosts || ""}
                                placeholder="Block hosts, e.g. example.com|spam.com"
                                onChange={(event) => updateProvider(id, { ...provider, block_hosts: event.target.value })}
                                onBlur={(event) => saveProvider(id, { ...provider, block_hosts: event.target.value })}
                              />
                            </Col>
                            <Col flex="120px">
                              <InputNumber
                                size="small"
                                min={0}
                                max={1}
                                value={provider.auth_info_level}
                                placeholder="Auth"
                                onChange={(auth_info_level) =>
                                  saveProvider(id, {
                                    ...provider,
                                    auth_info_level: auth_info_level === null ? undefined : auth_info_level,
                                  })
                                }
                              />
                            </Col>
                            <Col flex="140px">
                              <Select
                                allowClear
                                size="small"
                                value={provider.industry}
                                placeholder="Industry"
                                style={{ width: "100%" }}
                                onChange={(industry) => saveProvider(id, { ...provider, industry })}
                                options={[
                                  { label: "finance", value: "finance" },
                                  { label: "game", value: "game" },
                                ]}
                              />
                            </Col>
                          </Row>
                        </>
                      )}
                    </Space>
                  }
                />
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card size="small" title="Knowledge Store">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Row gutter={12}>
            <Col flex="auto">
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onPressEnter={searchKnowledge}
                placeholder="Search saved research knowledge"
                size="small"
              />
            </Col>
            <Col>
              <Button size="small" onClick={searchKnowledge}>
                Search
              </Button>
            </Col>
          </Row>
          <Row gutter={12} align="middle">
            <Col>
              <Text>Retention days</Text>
            </Col>
            <Col>
              <InputNumber
                size="small"
                min={1}
                value={research.cache?.retention_days}
                onChange={(retention_days) =>
                  onChange({
                    ...research,
                    cache: {
                      ...(research.cache || { enabled: true }),
                      retention_days: retention_days || 180,
                    },
                  })
                }
                onBlur={() => saveResearch(research)}
              />
            </Col>
          </Row>
          {results.length > 0 && (
            <List
              size="small"
              dataSource={results}
              renderItem={(item) => (
                <List.Item>
                  <List.Item.Meta
                    title={
                      <a href={item.url} target="_blank" rel="noreferrer">
                        {item.title}
                      </a>
                    }
                    description={item.summary || item.matched_text}
                  />
                </List.Item>
              )}
            />
          )}
        </Space>
      </Card>

      <Card
        size="small"
        title="Reports"
        extra={
          <Button size="small" icon={<ReloadOutlined />} onClick={loadReports}>
            Refresh
          </Button>
        }
      >
        {reports.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No research reports yet" />
        ) : (
          <List
            size="small"
            dataSource={reports.slice(0, 10)}
            renderItem={(report) => (
              <List.Item>
                <Space direction="vertical" size={0}>
                  <Text strong>{report.task_id}</Text>
                  <Text type="secondary" style={{ color: token.colorTextSecondary }}>
                    {report.date} - {report.path}
                  </Text>
                </Space>
              </List.Item>
            )}
          />
        )}
      </Card>

      <Card size="small" title="Tasks">
        {(research.tasks || []).length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="No scheduled research tasks configured" />
        ) : (
          <List
            dataSource={research.tasks}
            renderItem={(task) => (
              <List.Item>
                <List.Item.Meta
                  title={
                    <Space>
                      <Text strong>{task.name}</Text>
                      <Tag color={task.enabled ? "green" : "default"}>
                        {task.enabled ? "enabled" : "paused"}
                      </Tag>
                    </Space>
                  }
                  description={`${task.queries.length} queries - ${task.sources.join(", ")}`}
                />
              </List.Item>
            )}
          />
        )}
      </Card>
    </Space>
  );
}
