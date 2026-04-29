import { useCallback, useEffect, useMemo, useState, type CSSProperties } from "react";
import { ArrowLeftOutlined, ReloadOutlined } from "@ant-design/icons";
import { Alert, Button, Empty, Input, Progress, Space, Tag, Typography, message } from "antd";
import {
  getDevtoolsFrontendStatus,
  installDevtoolsFrontend,
  listDevtoolsPages,
  openDevtoolsSession,
  openSystemDevtoolsFrontend,
  type DebugPage,
  type DebugSession,
  type DevtoolsFrontendStatus,
} from "../../api/devtools";
import { buildBackendUrl } from "../../runtime";
import { copyToClipboard } from "../../utils/clipboard";

const { Text, Title } = Typography;

export default function DevTools() {
  const [pages, setPages] = useState<DebugPage[]>([]);
  const [query, setQuery] = useState("");
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [session, setSession] = useState<DebugSession | null>(null);
  const [loading, setLoading] = useState(false);
  const [frontendStatus, setFrontendStatus] = useState<DevtoolsFrontendStatus | null>(null);
  const [installingFrontend, setInstallingFrontend] = useState(false);
  const [installProgress, setInstallProgress] = useState<number | null>(null);

  const refreshPages = useCallback(async () => {
    const next = await listDevtoolsPages(true);
    setPages(next);
  }, []);

  useEffect(() => {
    void refreshPages();
    void refreshFrontendStatus();
    const timer = window.setInterval(() => {
      void refreshPages();
    }, 1500);
    return () => window.clearInterval(timer);
  }, [refreshPages]);

  const filteredPages = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return pages;
    return pages.filter((page) =>
      [page.title, page.url, page.adapter, page.state]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(needle)),
    );
  }, [pages, query]);

  const selectedPage = pages.find((page) => page.page_id === selectedPageId) ?? null;
  const cdpWebSocketUrl = selectedPage ? buildCdpWebSocketUrl(selectedPage.page_id) : null;
  const systemChromeFrontendUrl = cdpWebSocketUrl
    ? `devtools://devtools/bundled/inspector.html?ws=${cdpWebSocketUrl.replace(/^wss?:\/\//, "")}`
    : null;
  const embeddedChromeFrontendUrl =
    selectedPage && cdpWebSocketUrl && frontendStatus?.installed
      ? buildBackendUrl(
          `/api/devtools/frontend/inspector.html?ws=${cdpWebSocketUrl.replace(/^wss?:\/\//, "")}`,
        )
      : null;
  const canOpenBundledChromeFrontend = isChromiumBrowser();

  async function refreshFrontendStatus() {
    try {
      setFrontendStatus(await getDevtoolsFrontendStatus());
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to load DevTools frontend status",
      );
    }
  }

  const openPage = async (page: DebugPage) => {
    setSelectedPageId(page.page_id);
    setLoading(true);
    try {
      const nextSession = await openDevtoolsSession(page.page_id);
      setSession(nextSession);
    } catch (error) {
      message.error(error instanceof Error ? error.message : "Failed to open DevTools session");
    } finally {
      setLoading(false);
    }
  };

  const installFrontend = async () => {
    setInstallingFrontend(true);
    setInstallProgress(12);
    const progressTimer = window.setInterval(() => {
      setInstallProgress((current) => {
        if (current == null) return 12;
        return Math.min(current + 11, 88);
      });
    }, 800);
    try {
      const status = await installDevtoolsFrontend();
      window.clearInterval(progressTimer);
      setInstallProgress(100);
      setFrontendStatus(status);
      message.success("Chrome DevTools frontend is ready");
    } catch (error) {
      window.clearInterval(progressTimer);
      setInstallProgress(null);
      message.error(
        error instanceof Error ? error.message : "Failed to install Chrome DevTools frontend",
      );
    } finally {
      setInstallingFrontend(false);
    }
  };

  const copyDebugUrl = async () => {
    if (!systemChromeFrontendUrl) return;
    const ok = await copyToClipboard(systemChromeFrontendUrl);
    if (ok) {
      message.success("Debug URL copied");
    } else {
      message.error("Copy failed");
    }
  };

  const openInSystemChrome = async () => {
    if (!selectedPage) return;
    try {
      await openSystemDevtoolsFrontend(selectedPage.page_id);
      message.success("Chrome DevTools opened");
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to open Chrome DevTools",
      );
    }
  };

  if (!selectedPage) {
    return (
      <div style={pageShellStyle}>
        <Space direction="vertical" size={16} style={{ width: "100%", minHeight: 0 }}>
          <div style={listHeaderStyle}>
            <Space direction="vertical" size={4} style={{ minWidth: 0 }}>
              <Title level={3} style={{ margin: 0 }}>DevTools</Title>
              <Text type="secondary">Online pages that matched a devtools:// rule</Text>
            </Space>
            <Space.Compact style={{ width: "min(520px, 100%)" }}>
              <Input.Search
                allowClear
                placeholder="Search online pages"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onSearch={() => void refreshPages()}
              />
              <Button icon={<ReloadOutlined />} onClick={() => void refreshPages()}>
                Refresh Pages
              </Button>
            </Space.Compact>
          </div>

          <div data-testid="devtools-page-list" style={cardGridStyle}>
            {filteredPages.length === 0 ? (
              <div style={emptyListStyle}>
                <Empty description="No online pages" />
              </div>
            ) : (
              filteredPages.map((page) => (
                <button
                  key={page.page_id}
                  type="button"
                  data-testid="devtools-page-card"
                  onClick={() => void openPage(page)}
                  style={pageCardStyle}
                >
                  <Space direction="vertical" size={10} style={{ width: "100%", minWidth: 0 }}>
                    <Space direction="vertical" size={2} style={{ width: "100%", minWidth: 0 }}>
                      <Text strong ellipsis style={{ maxWidth: "100%", fontSize: 16 }}>
                        {page.title || "(untitled)"}
                      </Text>
                      <Text type="secondary" ellipsis style={{ maxWidth: "100%" }}>
                        {page.url}
                      </Text>
                    </Space>
                    <Space wrap size={6}>
                      <Tag color="blue">{page.adapter}</Tag>
                      <Tag color="gold">{page.fidelity}</Tag>
                      <Tag>{page.state}</Tag>
                      <Tag>{page.mode}</Tag>
                    </Space>
                  </Space>
                </button>
              ))
            )}
          </div>
        </Space>
      </div>
    );
  }

  return (
    <div data-testid="devtools-detail" style={detailShellStyle}>
      <Space direction="vertical" size={12} style={{ width: "100%", height: "100%", minHeight: 0 }}>
        <div style={detailHeaderStyle}>
          <Button
            size="small"
            data-testid="devtools-back"
            icon={<ArrowLeftOutlined />}
            onClick={() => setSelectedPageId(null)}
          >
            Back
          </Button>
          <Space size={8} style={{ minWidth: 0, flex: 1 }}>
            <Text strong ellipsis style={{ maxWidth: 220, fontSize: 16 }}>
              {selectedPage.title || "(untitled)"}
            </Text>
            <Text type="secondary" ellipsis style={{ maxWidth: "min(720px, 55vw)" }}>
              {selectedPage.url}
            </Text>
          </Space>
          <Tag>{session?.state ?? selectedPage.state}</Tag>
        </div>

        <div style={{ flex: 1, minHeight: 0, overflow: "auto" }}>
          {embeddedChromeFrontendUrl ? (
          <iframe
            title="Chrome DevTools Frontend"
            src={embeddedChromeFrontendUrl}
            style={frontendFrameStyle}
          />
        ) : (
          <Space direction="vertical" size={16} style={detailsPanelStyle}>
            <div style={{ display: "flex", justifyContent: "space-between", gap: 12 }}>
              <Space direction="vertical" size={4} style={{ minWidth: 0 }}>
                <Title level={4} style={{ margin: 0 }}>{selectedPage.title || "(untitled)"}</Title>
                <Text type="secondary" ellipsis>{selectedPage.url}</Text>
              </Space>
              <Space>
                <Tag>{session?.state ?? selectedPage.state}</Tag>
              </Space>
            </div>

            {selectedPage.status_reason ? (
              <Alert type="warning" showIcon message={selectedPage.status_reason} />
            ) : null}

            <Space direction="vertical" size={8} style={{ width: "100%" }}>
              <Text strong>Debug URL</Text>
              <Input.TextArea
                readOnly
                autoSize={{ minRows: 2, maxRows: 4 }}
                value={systemChromeFrontendUrl ?? ""}
              />
              <Text type="secondary">
                Copy this address and open it in Chrome or Edge to use the browser's built-in DevTools frontend.
              </Text>
            </Space>

            <Space wrap>
              <Button onClick={() => void copyDebugUrl()} disabled={!systemChromeFrontendUrl}>
                Copy Debug URL
              </Button>
              {canOpenBundledChromeFrontend && systemChromeFrontendUrl ? (
                <Button type="primary" onClick={() => void openInSystemChrome()}>
                  Open in Chrome DevTools
                </Button>
              ) : null}
              <Button loading={loading} onClick={() => selectedPage && void openPage(selectedPage)}>
                Refresh
              </Button>
            </Space>

            <div style={installPanelStyle}>
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <Text strong>Embedded Chrome DevTools</Text>
                <Text type="secondary">
                  Optional. Bifrost downloads the official frontend only after you click install, then shows it here.
                </Text>
                <Space>
                  <Button loading={installingFrontend} onClick={() => void installFrontend()}>
                    Install Chrome DevTools
                  </Button>
                  <Button onClick={() => void refreshFrontendStatus()}>Refresh Status</Button>
                </Space>
                {installingFrontend || installProgress !== null ? (
                  <Progress
                    percent={installProgress ?? 0}
                    status={installProgress === 100 ? "success" : "active"}
                  />
                ) : null}
                {frontendStatus?.state === "broken" ? (
                  <Alert type="warning" showIcon message={frontendStatus.reason} />
                ) : null}
              </Space>
            </div>
          </Space>
          )}
        </div>
      </Space>
    </div>
  );
}

const pageShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: 20,
  overflow: "auto",
  background: "#f7f9fc",
};

const detailShellStyle: CSSProperties = {
  height: "100%",
  minHeight: 0,
  padding: "10px 20px 12px",
  overflow: "auto",
  background: "#f7f9fc",
};

const listHeaderStyle: CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "flex-start",
  gap: 16,
  flexWrap: "wrap",
};

const cardGridStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fill, minmax(280px, 1fr))",
  gap: 14,
};

const pageCardStyle: CSSProperties = {
  appearance: "none",
  WebkitAppearance: "none",
  display: "block",
  width: "100%",
  minHeight: 136,
  padding: 16,
  border: "1px solid #d9e2ef",
  borderRadius: 8,
  background: "#fff",
  color: "inherit",
  textAlign: "left",
  cursor: "pointer",
  boxShadow: "0 1px 2px rgba(15, 23, 42, 0.04)",
};

const emptyListStyle: CSSProperties = {
  gridColumn: "1 / -1",
  minHeight: 320,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
};

const detailHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 12,
  minWidth: 0,
  minHeight: 28,
};

const detailsPanelStyle: CSSProperties = {
  width: "100%",
  maxWidth: 960,
  margin: "0 auto",
};

const installPanelStyle: CSSProperties = {
  padding: 12,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  background: "#fff",
};

const frontendFrameStyle: CSSProperties = {
  width: "100%",
  height: "calc(100vh - 80px)",
  minHeight: 620,
  border: "1px solid #d9e2ef",
  borderRadius: 6,
  background: "#fff",
};

function buildCdpWebSocketUrl(pageId: string): string {
  const url = new URL(buildBackendUrl(`/api/devtools/cdp/${pageId}`));
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function isChromiumBrowser(): boolean {
  const ua = navigator.userAgent;
  return /(Chrome|Chromium|Edg)\//.test(ua) && !/(OPR|Opera)\//.test(ua);
}
