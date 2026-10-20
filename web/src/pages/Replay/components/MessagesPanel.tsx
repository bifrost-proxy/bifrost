import { useCallback, useMemo, type CSSProperties } from "react";
import { Input, Button, Empty, Tag, Tooltip, theme, Typography, Space, Switch } from "antd";
import {
  SendOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  ClockCircleOutlined,
  SearchOutlined,
  FilterOutlined,
  HighlightOutlined,
  FullscreenOutlined,
  VerticalAlignBottomOutlined,
  ArrowUpOutlined,
  ArrowDownOutlined,
} from "@ant-design/icons";
import { useReplayStore } from "../../../stores/useReplayStore";
import {
  VirtualMessageList,
  MessageItemCard,
  useMessageSearch,
  FullscreenMessageViewer,
  normalizeSSEEvent,
  normalizeWSMessage,
  type MessageItem,
} from "../../../components/VirtualMessageViewer";
import { useI18n, type TranslationKey } from "../../../i18n";

const { Text } = Typography;
const { Search } = Input;

function formatDuration(startedAt: number, endedAt?: number): string {
  const duration = (endedAt || Date.now()) - startedAt;
  if (duration < 1000) return `${duration}ms`;
  if (duration < 60000) return `${(duration / 1000).toFixed(1)}s`;
  return `${Math.floor(duration / 60000)}m ${Math.floor((duration % 60000) / 1000)}s`;
}

const STATUS_LABEL_KEYS: Record<string, TranslationKey> = {
  connecting: "replay.messages.status.connecting",
  connected: "replay.messages.status.connected",
  disconnected: "replay.messages.status.disconnected",
  error: "replay.messages.status.error",
};

export default function MessagesPanel() {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const {
    streamingConnection,
    sseEvents,
    wsMessages,
    uiState,
    disconnectSSE,
    disconnectWebSocket,
    sendWebSocketMessage,
    clearStreamingMessages,
    updateUIState,
  } = useReplayStore();

  const isSSE = streamingConnection?.type === "sse";
  const isWebSocket = streamingConnection?.type === "websocket";
  const isConnected = streamingConnection?.status === "connected";
  const messageInput = uiState.wsMessageInput;
  const searchQuery = uiState.messagesPanelSearch;
  const searchMode = uiState.messagesPanelSearchMode;
  const fullscreenOpen = uiState.messagesPanelFullscreen;
  const followTail = uiState.messagesPanelFollowTail;

  const normalizedMessages = useMemo<MessageItem[]>(() => {
    if (isSSE) {
      return sseEvents.map((event, index) => normalizeSSEEvent(event, index));
    }
    if (isWebSocket) {
      return wsMessages.map(normalizeWSMessage);
    }
    return [];
  }, [isSSE, isWebSocket, sseEvents, wsMessages]);

  const {
    searchState,
    setQuery,
    setMatchMode,
    filteredItems,
    highlightedIndices,
    goToNext,
    goToPrev,
    matchTokens,
  } = useMessageSearch({
    items: normalizedMessages,
    initialQuery: searchQuery,
    initialMatchMode: searchMode,
  });

  const displayItems = searchMode === 'filter' && searchQuery ? filteredItems : normalizedMessages;

  const handleSearchChange = useCallback((value: string) => {
    setQuery(value);
    updateUIState({ messagesPanelSearch: value });
  }, [setQuery, updateUIState]);

  const handleModeChange = useCallback((mode: 'highlight' | 'filter') => {
    setMatchMode(mode);
    updateUIState({ messagesPanelSearchMode: mode });
  }, [setMatchMode, updateUIState]);

  const handleFullscreenOpen = useCallback(() => {
    updateUIState({ messagesPanelFullscreen: true });
  }, [updateUIState]);

  const handleFullscreenClose = useCallback(() => {
    updateUIState({ messagesPanelFullscreen: false });
  }, [updateUIState]);

  const handleFollowTailChange = useCallback((following: boolean) => {
    updateUIState({ messagesPanelFollowTail: following });
  }, [updateUIState]);

  const handleDisconnect = useCallback(() => {
    if (isSSE) {
      disconnectSSE();
    } else if (isWebSocket) {
      disconnectWebSocket();
    }
  }, [isSSE, isWebSocket, disconnectSSE, disconnectWebSocket]);

  const handleSendMessage = useCallback(() => {
    if (messageInput.trim()) {
      sendWebSocketMessage(messageInput);
      updateUIState({ wsMessageInput: "" });
    }
  }, [messageInput, sendWebSocketMessage, updateUIState]);

  const handleKeyPress = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSendMessage();
      }
    },
    [handleSendMessage]
  );

  const styles: Record<string, CSSProperties> = {
    container: {
      height: "100%",
      display: "flex",
      flexDirection: "column",
      overflow: "hidden",
    },
    header: {
      padding: "8px 12px",
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      alignItems: "center",
      justifyContent: "space-between",
      gap: 8,
      flexShrink: 0,
    },
    connectionInfo: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      flex: 1,
      minWidth: 0,
    },
    toolbar: {
      padding: "6px 12px",
      borderBottom: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      alignItems: "center",
      gap: 8,
      flexShrink: 0,
      flexWrap: "wrap",
    },
    messageList: {
      flex: 1,
      overflow: "hidden",
      minHeight: 0,
    },
    inputArea: {
      padding: "8px 12px",
      borderTop: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      gap: 8,
      flexShrink: 0,
    },
  };

  const getItemKey = useCallback((item: MessageItem) => item.id, []);

  const renderItem = useCallback((item: MessageItem) => (
    <MessageItemCard
      message={item}
      searchTokens={searchMode === 'highlight' ? matchTokens : []}
    />
  ), [searchMode, matchTokens]);

  if (!streamingConnection) {
    return (
      <div style={styles.container}>
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={t("replay.messages.noConnection")}
          style={{ margin: "auto" }}
        />
      </div>
    );
  }

  const statusColors: Record<string, string> = {
    connecting: "processing",
    connected: "success",
    disconnected: "default",
    error: "error",
  };

  const matchInfo = searchState.total > 0 
    ? `${searchState.currentIndex >= 0 ? searchState.currentIndex + 1 : 0}/${searchState.total}`
    : null;
  const currentHighlightIndex =
    searchState.currentIndex >= 0
      ? searchMode === "filter"
        ? searchState.currentIndex
        : searchState.matchedIndices[searchState.currentIndex] ?? -1
      : -1;

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <div style={styles.connectionInfo}>
          <Tag color={isSSE ? "orange" : "purple"}>
            {isSSE ? "SSE" : "WebSocket"}
          </Tag>
          <Tag color={statusColors[streamingConnection.status]}>
            {STATUS_LABEL_KEYS[streamingConnection.status]
              ? t(STATUS_LABEL_KEYS[streamingConnection.status])
              : streamingConnection.status}
          </Tag>
          <Tooltip title={streamingConnection.url}>
            <Text
              ellipsis
              style={{ fontSize: 11, maxWidth: 200 }}
              type="secondary"
            >
              {streamingConnection.url}
            </Text>
          </Tooltip>
          <Text type="secondary" style={{ fontSize: 11 }}>
            <ClockCircleOutlined style={{ marginRight: 4 }} />
            {formatDuration(
              streamingConnection.startedAt,
              streamingConnection.endedAt
            )}
          </Text>
        </div>
        <div style={{ display: "flex", gap: 4 }}>
          <Tooltip title={t("replay.messages.fullscreen")}>
            <Button
              size="small"
              icon={<FullscreenOutlined />}
              onClick={handleFullscreenOpen}
              disabled={normalizedMessages.length === 0}
            />
          </Tooltip>
          <Tooltip title={t("replay.messages.clear")}>
            <Button
              size="small"
              icon={<DeleteOutlined />}
              onClick={clearStreamingMessages}
            />
          </Tooltip>
          {isConnected && (
            <Tooltip title={t("replay.request.disconnect")}>
              <Button
                size="small"
                danger
                icon={<DisconnectOutlined />}
                onClick={handleDisconnect}
              />
            </Tooltip>
          )}
        </div>
      </div>

      <div style={styles.toolbar}>
        <Search
          placeholder={t("replay.messages.searchPlaceholder")}
          allowClear
          value={searchQuery}
          onChange={(e) => handleSearchChange(e.target.value)}
          onSearch={handleSearchChange}
          style={{ width: 200 }}
          size="small"
          prefix={<SearchOutlined />}
        />

        <Button.Group size="small">
          <Tooltip title={t("replay.messages.highlightMatches")}>
            <Button
              type={searchMode === 'highlight' ? 'primary' : 'default'}
              icon={<HighlightOutlined />}
              onClick={() => handleModeChange('highlight')}
            />
          </Tooltip>
          <Tooltip title={t("replay.messages.filterMatches")}>
            <Button
              type={searchMode === 'filter' ? 'primary' : 'default'}
              icon={<FilterOutlined />}
              onClick={() => handleModeChange('filter')}
            />
          </Tooltip>
        </Button.Group>

        {matchInfo && (
          <>
            <Tag color="blue" style={{ margin: 0 }}>{matchInfo}</Tag>
            <Button.Group size="small">
              <Button icon={<ArrowUpOutlined />} onClick={goToPrev} />
              <Button icon={<ArrowDownOutlined />} onClick={goToNext} />
            </Button.Group>
          </>
        )}

        <Tooltip title={t("replay.messages.followLatest")}>
          <Space size={4}>
            <VerticalAlignBottomOutlined style={{ fontSize: 12, color: token.colorTextSecondary }} />
            <Switch
              size="small"
              checked={followTail}
              onChange={handleFollowTailChange}
            />
          </Space>
        </Tooltip>

        <Text type="secondary" style={{ fontSize: 11, marginLeft: 'auto' }}>
          {t("replay.messages.count", { count: displayItems.length })}
        </Text>
      </div>

      <div style={styles.messageList}>
        <VirtualMessageList
          items={displayItems}
          getItemKey={getItemKey}
          renderItem={renderItem}
          highlightedIndices={searchMode === 'highlight' ? highlightedIndices : []}
          currentHighlightIndex={currentHighlightIndex}
          estimateSize={120}
          overscan={3}
          followTail={followTail}
          onFollowTailChange={handleFollowTailChange}
          showScrollControls
          scrollTopTestId="replay-messages-scroll-top"
          scrollBottomTestId="replay-messages-scroll-bottom"
          emptyContent={
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description={
                searchMode === 'filter' && searchQuery
                  ? t("replay.messages.noMatch")
                  : isConnected
                    ? t("replay.messages.waiting")
                    : t("replay.messages.empty")
              }
              style={{ marginTop: 40 }}
            />
          }
          style={{ padding: "8px 12px" }}
        />
      </div>

      {isWebSocket && isConnected && (
        <div style={styles.inputArea}>
          <Input.TextArea
            placeholder={t("replay.messages.inputPlaceholder")}
            autoSize={{ minRows: 1, maxRows: 4 }}
            value={messageInput}
            onChange={(e) => updateUIState({ wsMessageInput: e.target.value })}
            onKeyPress={handleKeyPress}
            style={{ flex: 1 }}
          />
          <Button
            type="primary"
            icon={<SendOutlined />}
            onClick={handleSendMessage}
            disabled={!messageInput.trim()}
          >
            {t("common.send")}
          </Button>
        </div>
      )}

      {streamingConnection.error && (
        <div
          style={{
            padding: "8px 12px",
            backgroundColor: token.colorErrorBg,
            color: token.colorError,
            fontSize: 12,
          }}
        >
          {t("replay.messages.error", { error: streamingConnection.error })}
        </div>
      )}

      <FullscreenMessageViewer
        open={fullscreenOpen}
        onClose={handleFullscreenClose}
        items={normalizedMessages}
        title={
          <Space>
            <Tag color={isSSE ? "orange" : "purple"}>
              {isSSE ? "SSE" : "WebSocket"}
            </Tag>
            <Text type="secondary">{streamingConnection.url}</Text>
          </Space>
        }
        initialQuery={searchQuery}
        initialMatchMode={searchMode}
        followTail={followTail}
        onFollowTailChange={handleFollowTailChange}
      />
    </div>
  );
}
