import { useMemo, useCallback, useEffect, useRef, useState } from 'react';
import { Typography, Button, Space, Tooltip, Input, Tag, theme } from 'antd';
import {
  ReloadOutlined,
  SearchOutlined,
  FilterOutlined,
  HighlightOutlined,
  ArrowUpOutlined,
  ArrowDownOutlined,
  FullscreenOutlined,
  LoadingOutlined,
} from '@ant-design/icons';
import { useVirtualizer } from '@tanstack/react-virtual';
import type { SSEEvent } from '../../../../types';
import { SseEventCard } from './SseEventCard';
import { useI18n } from '../../../../i18n';

const { Text } = Typography;
const { Search } = Input;
const MAX_SSE_SEARCHABLE_TEXT_LENGTH = 4 * 1024;

interface SseMessageListProps {
  events: SSEEvent[];
  loading: boolean;
  hasMore: boolean;
  searchQuery?: string;
  searchMode?: 'highlight' | 'filter';
  onSearchChange?: (query: string) => void;
  onSearchModeChange?: (mode: 'highlight' | 'filter') => void;
  onLoadMore: () => void;
  onRefresh: () => void;
  onFullscreenOpen?: () => void;
  connectionState?: 'idle' | 'connecting' | 'open' | 'closed' | 'error';
  externalNext?: number;
  onMatchCountChange?: (total: number) => void;
  onMatchNavigate?: (next: number) => void;
  onOpenDetail?: (event: SSEEvent) => void;
}

export const SseMessageList = ({
  events,
  loading,
  hasMore,
  searchQuery = '',
  searchMode = 'highlight',
  onSearchChange,
  onSearchModeChange,
  onLoadMore,
  onRefresh,
  onFullscreenOpen,
  connectionState,
  externalNext,
  onMatchCountChange,
  onMatchNavigate,
  onOpenDetail,
}: SseMessageListProps) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const parentRef = useRef<HTMLDivElement>(null);
  const [currentMatch, setCurrentMatch] = useState<number>(-1);
  const normalizedQuery = searchQuery.trim().toLowerCase();
  const [isAtTop, setIsAtTop] = useState(true);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const scrollButtonStyles = useMemo(
    () => ({
      scrollButton: {
        position: "absolute" as const,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: 36,
        height: 36,
        backgroundColor: token.colorBgElevated,
        color: token.colorTextSecondary,
        borderRadius: "50%",
        cursor: "pointer",
        boxShadow: "0 2px 8px rgba(0, 0, 0, 0.08)",
        zIndex: 10,
        border: `1px solid ${token.colorBorderSecondary}`,
        transition:
          "opacity 0.3s ease, transform 0.3s ease, background-color 0.2s",
      },
      scrollToTopButton: {
        top: 16,
        left: "50%",
        transform: "translateX(-50%)",
      },
      scrollToBottomButton: {
        bottom: 16,
        left: "50%",
        transform: "translateX(-50%)",
      },
    }),
    [token],
  );

  const getSearchableText = useCallback((event: SSEEvent) => {
    const data = event.data || '';
    const limitedData = data.length > MAX_SSE_SEARCHABLE_TEXT_LENGTH
      ? data.slice(0, MAX_SSE_SEARCHABLE_TEXT_LENGTH)
      : data;
    return `${event.event || 'message'} ${event.id || ''} ${limitedData}`.toLowerCase();
  }, []);

  const displayEvents = useMemo(() => {
    if (searchMode !== 'filter' || !normalizedQuery) return events;
    return events.filter((event) => {
      const text = getSearchableText(event);
      return text.includes(normalizedQuery);
    });
  }, [events, getSearchableText, normalizedQuery, searchMode]);

  const matchedIndices = useMemo(() => {
    if (!normalizedQuery) return [];
    const indices: number[] = [];
    displayEvents.forEach((event, index) => {
      const text = getSearchableText(event);
      if (text.includes(normalizedQuery)) {
        indices.push(index);
      }
    });
    return indices;
  }, [displayEvents, getSearchableText, normalizedQuery]);

  const getEventKey = useCallback((event: SSEEvent, index: number) => {
    return `sse-${index}-${event.id || ''}-${event.timestamp}`;
  }, []);

  const getItemKey = useCallback((index: number) => {
    const event = displayEvents[index];
    if (!event) return String(index);
    return getEventKey(event, index);
  }, [displayEvents, getEventKey]);

  const rowVirtualizer = useVirtualizer({
    count: displayEvents.length,
    getScrollElement: () => parentRef.current,
    getItemKey,
    estimateSize: () => 160,
    overscan: 6,
  });

  const handleScrollToTop = useCallback(() => {
    if (displayEvents.length === 0) return;
    rowVirtualizer.scrollToIndex(0, { align: "start" });
  }, [displayEvents.length, rowVirtualizer]);

  const handleScrollToBottom = useCallback(() => {
    if (displayEvents.length === 0) return;
    rowVirtualizer.scrollToIndex(displayEvents.length - 1, { align: "end" });
  }, [displayEvents.length, rowVirtualizer]);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const onScroll = () => {
      const threshold = 8;
      const distanceToBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
      setIsAtTop(el.scrollTop <= threshold);
      setIsAtBottom(distanceToBottom <= threshold);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
    return () => el.removeEventListener("scroll", onScroll);
  }, [displayEvents.length]);


  useEffect(() => {
    if (matchedIndices.length === 0) {
      setCurrentMatch(-1);
      return;
    }
    setCurrentMatch(0);
    rowVirtualizer.scrollToIndex(matchedIndices[0], { align: 'center' });
  }, [matchedIndices, rowVirtualizer]);

  const lastReportedTotalRef = useRef<number | null>(null);
  useEffect(() => {
    if (!onMatchCountChange) return;
    const nextTotal = matchedIndices.length;
    if (lastReportedTotalRef.current === nextTotal) return;
    lastReportedTotalRef.current = nextTotal;
    onMatchCountChange(nextTotal);
  }, [matchedIndices.length, onMatchCountChange]);

  const prevExternalNext = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!externalNext || matchedIndices.length === 0) {
      prevExternalNext.current = externalNext;
      return;
    }
    if (prevExternalNext.current === externalNext) return;
    prevExternalNext.current = externalNext;
    const idx = (externalNext - 1) % matchedIndices.length;
    setCurrentMatch(idx);
    rowVirtualizer.scrollToIndex(matchedIndices[idx], { align: 'center' });
  }, [externalNext, matchedIndices, rowVirtualizer]);

  const handleSearchChange = useCallback((value: string) => {
    onSearchChange?.(value);
  }, [onSearchChange]);

  const handleModeChange = useCallback((mode: 'highlight' | 'filter') => {
    onSearchModeChange?.(mode);
  }, [onSearchModeChange]);

  const goToPrev = useCallback(() => {
    if (matchedIndices.length === 0) return;
    const nextIndex =
      currentMatch <= 0 ? matchedIndices.length - 1 : currentMatch - 1;
    setCurrentMatch(nextIndex);
    rowVirtualizer.scrollToIndex(matchedIndices[nextIndex], { align: 'center' });
    onMatchNavigate?.(nextIndex + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentMatch, matchedIndices, rowVirtualizer]);

  const goToNext = useCallback(() => {
    if (matchedIndices.length === 0) return;
    const nextIndex =
      currentMatch >= matchedIndices.length - 1 ? 0 : currentMatch + 1;
    setCurrentMatch(nextIndex);
    rowVirtualizer.scrollToIndex(matchedIndices[nextIndex], { align: 'center' });
    onMatchNavigate?.(nextIndex + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentMatch, matchedIndices, rowVirtualizer]);

  const matchInfo = matchedIndices.length > 0
    ? `${currentMatch >= 0 ? currentMatch + 1 : 0}/${matchedIndices.length}`
    : null;

  const stateLabel = (() => {
    if (connectionState === 'open') return t('trafficDetail.messages.stateLive');
    if (connectionState === 'closed') return t('trafficDetail.messages.stateClosed');
    if (connectionState === 'error') return t('trafficDetail.messages.stateError');
    if (connectionState === 'connecting') return t('trafficDetail.messages.stateConnecting');
    return t('trafficDetail.messages.stateIdle');
  })();

  const stateColor = (() => {
    if (connectionState === 'open') return 'green';
    if (connectionState === 'closed') return 'default';
    if (connectionState === 'error') return 'red';
    if (connectionState === 'connecting') return 'blue';
    return 'default';
  })();

  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        minHeight: 0,
      }}
      data-testid="sse-message-container"
    >
      <div
        style={{
          marginBottom: 8,
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          gap: 8,
          flexShrink: 0,
          flexWrap: 'wrap',
        }}
      >
        <Space size="small">
          <Search
            placeholder={t('trafficDetail.messages.searchEvents')}
            allowClear
            value={searchQuery}
            onChange={(e) => handleSearchChange(e.target.value)}
            onSearch={handleSearchChange}
            style={{ width: 180 }}
            size="small"
            prefix={<SearchOutlined />}
          />

          <Button.Group size="small">
            <Tooltip title={t('trafficDetail.messages.highlightMatches')}>
              <Button
                type={searchMode === 'highlight' ? 'primary' : 'default'}
                icon={<HighlightOutlined />}
                onClick={() => handleModeChange('highlight')}
              />
            </Tooltip>
            <Tooltip title={t('trafficDetail.messages.filterMatchesOnly')}>
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

          <Tag color={stateColor} style={{ margin: 0 }}>
            {connectionState === 'connecting' || loading ? (
              <Space size={4}>
                <LoadingOutlined />
                {stateLabel}
              </Space>
            ) : (
              stateLabel
            )}
          </Tag>

          <Text
            type="secondary"
            style={{ fontSize: 11 }}
            data-testid="sse-message-count"
          >
            {t('trafficDetail.messages.eventCount', { shown: displayEvents.length, total: events.length })}
            {hasMore && ' (+)'}
          </Text>
        </Space>

        <Space size="small">
          {onFullscreenOpen && (
            <Tooltip title={t('trafficDetail.messages.fullscreen')}>
              <Button
                size="small"
                icon={<FullscreenOutlined />}
                onClick={onFullscreenOpen}
              />
            </Tooltip>
          )}
          {hasMore && (
            <Button size="small" onClick={onLoadMore} loading={loading}>
              {t('trafficDetail.messages.loadMore')}
            </Button>
          )}
          <Tooltip title={t('common.refresh')}>
            <Button
              size="small"
              icon={<ReloadOutlined />}
              onClick={onRefresh}
              loading={loading}
            />
          </Tooltip>
        </Space>
      </div>

      <div style={{ flex: 1, overflow: "hidden", minHeight: 0, position: "relative" }}>
        {displayEvents.length === 0 && !loading ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: token.colorTextSecondary,
            }}
          >
            {searchMode === 'filter' && searchQuery
              ? t('trafficDetail.messages.noEventsMatch')
              : t('trafficDetail.messages.noEvents')}
          </div>
        ) : (
          <div
            ref={parentRef}
            data-testid="sse-message-scroll"
            style={{
              height: '100%',
              overflow: 'auto',
              paddingRight: 8,
            }}
          >
            <div
              style={{
                height: rowVirtualizer.getTotalSize(),
                width: '100%',
                position: 'relative',
              }}
            >
              {rowVirtualizer.getVirtualItems().map((virtualRow) => {
                const event = displayEvents[virtualRow.index];
                if (!event) return null;
                const key = getEventKey(event, virtualRow.index);
                return (
                  <div
                    key={key}
                    ref={rowVirtualizer.measureElement}
                    data-index={virtualRow.index}
                    style={{
                      position: 'absolute',
                      top: 0,
                      left: 0,
                      width: '100%',
                      transform: `translateY(${virtualRow.start}px)`,
                      paddingBottom: 8,
                    }}
                  >
                    <SseEventCard
                      event={event}
                      index={virtualRow.index}
                      searchValue={searchMode === 'highlight' ? searchQuery : undefined}
                      onOpenDetail={() => onOpenDetail?.(event)}
                    />
                  </div>
                );
              })}
            </div>
          </div>
        )}
        {!isAtTop && (
          <div
            style={{
              ...scrollButtonStyles.scrollButton,
              ...scrollButtonStyles.scrollToTopButton,
            }}
            onClick={handleScrollToTop}
            data-testid="sse-scroll-top"
          >
            <ArrowUpOutlined style={{ fontSize: 14 }} />
          </div>
        )}
        {!isAtBottom && (
          <div
            style={{
              ...scrollButtonStyles.scrollButton,
              ...scrollButtonStyles.scrollToBottomButton,
            }}
            onClick={handleScrollToBottom}
            data-testid="sse-scroll-bottom"
          >
            <ArrowDownOutlined style={{ fontSize: 14 }} />
          </div>
        )}
      </div>
    </div>
  );
};
