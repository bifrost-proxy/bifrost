import { useState, useMemo, useCallback, type CSSProperties } from 'react';
import { Typography, Tag, Button, Space, Tooltip, theme } from 'antd';
import {
  CopyOutlined,
  CheckOutlined,
  ExpandOutlined,
  CompressOutlined,
  ArrowUpOutlined,
  ArrowDownOutlined,
} from '@ant-design/icons';
import dayjs from 'dayjs';
import hljs from 'highlight.js/lib/core';
import json from 'highlight.js/lib/languages/json';
import '../../styles/hljs-github-theme.css';
import type { MessageItem } from './types';
import { highlightText } from './useMessageSearch';
import { copyToClipboard } from '../../utils/clipboard';
import { useI18n, type TFunction } from '../../i18n';

hljs.registerLanguage('json', json);

const { Text } = Typography;

export interface MessageItemCardProps {
  message: MessageItem;
  searchTokens?: string[];
  caseSensitive?: boolean;
  onHeightChange?: () => void;
}

function formatTime(timestamp: number): string {
  return dayjs(timestamp).format('HH:mm:ss.SSS');
}

function highlightJson(text: string): string {
  try {
    const result = hljs.highlight(text, { language: 'json' });
    return result.value;
  } catch {
    return text;
  }
}

const getDirectionIcon = (direction: MessageItem['direction']) => {
  switch (direction) {
    case 'send':
      return <ArrowUpOutlined style={{ fontSize: 10 }} />;
    case 'receive':
      return <ArrowDownOutlined style={{ fontSize: 10 }} />;
    default:
      return <ArrowDownOutlined style={{ fontSize: 10 }} />;
  }
};

const getDirectionColor = (direction: MessageItem['direction'], token: ReturnType<typeof theme.useToken>['token']) => {
  switch (direction) {
    case 'send':
      return token.colorPrimary;
    case 'receive':
      return token.colorSuccess;
    default:
      return token.colorSuccess;
  }
};

const getDirectionLabel = (direction: MessageItem['direction'], t: TFunction) => {
  switch (direction) {
    case 'send':
      return t('trafficDetail.viewer.dirSent');
    case 'receive':
      return t('trafficDetail.viewer.dirReceived');
    case 'event':
      return t('trafficDetail.viewer.dirEvent');
  }
};

const getTypeColor = (type: MessageItem['type']) => {
  switch (type) {
    case 'json':
      return 'cyan';
    case 'text':
      return 'blue';
    case 'binary':
      return 'purple';
    case 'ping':
    case 'pong':
      return 'geekblue';
    case 'close':
      return 'red';
    case 'sse':
      return 'green';
    case 'continuation':
      return 'default';
    default:
      return 'default';
  }
};

export function MessageItemCard({
  message,
  searchTokens = [],
  caseSensitive = false,
  onHeightChange,
}: MessageItemCardProps) {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);

  const formattedContent = useMemo(() => {
    if (message.metadata?.isJson && message.data) {
      try {
        const parsed = JSON.parse(message.data);
        return JSON.stringify(parsed, null, 2);
      } catch {
        return message.data;
      }
    }
    return message.data;
  }, [message.data, message.metadata?.isJson]);

  const highlightedHtml = useMemo(() => {
    if (!message.metadata?.isJson) return null;
    return highlightJson(formattedContent);
  }, [message.metadata?.isJson, formattedContent]);

  const shouldTruncate = !expanded && formattedContent.split('\n').length > 8;
  const displayContent = shouldTruncate
    ? formattedContent.split('\n').slice(0, 8).join('\n') + '\n...'
    : formattedContent;

  const handleCopy = useCallback(async () => {
    const success = await copyToClipboard(formattedContent);
    if (success) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  }, [formattedContent]);

  const toggleExpanded = useCallback(() => {
    setExpanded(prev => !prev);
    setTimeout(() => {
      onHeightChange?.();
    }, 0);
  }, [onHeightChange]);

  const renderHighlightedText = useCallback((text: string) => {
    if (searchTokens.length === 0) return text;
    const { segments } = highlightText(text, searchTokens, caseSensitive);
    return segments.map((seg, i) =>
      seg.highlighted ? (
        <mark key={i} style={{ backgroundColor: '#ffe58f', padding: 0 }}>
          {seg.text}
        </mark>
      ) : (
        seg.text
      )
    );
  }, [searchTokens, caseSensitive]);

  const cardStyle: CSSProperties = {
    borderRadius: 6,
    border: `1px solid ${token.colorBorderSecondary}`,
    backgroundColor: token.colorBgContainer,
    overflow: 'hidden',
    marginBottom: 8,
  };

  const headerStyle: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: '6px 10px',
    backgroundColor: token.colorFillQuaternary,
    borderBottom: `1px solid ${token.colorBorderSecondary}`,
  };

  const bodyStyle: CSSProperties = {
    padding: '8px 10px',
  };

  return (
    <div style={cardStyle}>
      <div style={headerStyle}>
        <Space size={8} align="center">
          <span style={{ color: getDirectionColor(message.direction, token) }}>
            {getDirectionIcon(message.direction)}
          </span>
          <Text type="secondary" style={{ fontSize: 11, fontFamily: 'monospace' }}>
            {formatTime(message.timestamp)}
          </Text>
          <Tag
            color={message.direction === 'send' ? 'blue' : message.direction === 'receive' ? 'green' : 'orange'}
            style={{ margin: 0, fontSize: 11, lineHeight: '18px', padding: '0 6px' }}
          >
            {getDirectionLabel(message.direction, t)}
          </Tag>
          <Tag
            color={getTypeColor(message.type)}
            style={{ margin: 0, fontSize: 11, lineHeight: '18px', padding: '0 6px' }}
          >
            {message.type.toUpperCase()}
          </Tag>
          {message.metadata?.eventId && (
            <Text type="secondary" style={{ fontSize: 11 }}>
              id: {message.metadata.eventId}
            </Text>
          )}
          {message.metadata?.eventType && (
            <Tag color="purple" style={{ margin: 0, fontSize: 10, lineHeight: '16px', padding: '0 4px' }}>
              {message.metadata.eventType}
            </Tag>
          )}
          {message.metadata?.isJson && (
            <Tag color="cyan" style={{ margin: 0, fontSize: 10, lineHeight: '16px', padding: '0 4px' }}>
              JSON
            </Tag>
          )}
        </Space>
        <Space size={4}>
          <Tooltip title={copied ? t('trafficDetail.messages.copiedBang') : t('common.copy')}>
            <Button
              type="text"
              size="small"
              icon={copied ? <CheckOutlined style={{ color: token.colorSuccess }} /> : <CopyOutlined />}
              onClick={handleCopy}
              disabled={!message.data}
              style={{ width: 24, height: 24 }}
            />
          </Tooltip>
          {formattedContent.split('\n').length > 8 && (
            <Tooltip title={expanded ? t('trafficDetail.header.collapse') : t('trafficDetail.header.expand')}>
              <Button
                type="text"
                size="small"
                icon={expanded ? <CompressOutlined /> : <ExpandOutlined />}
                onClick={toggleExpanded}
                style={{ width: 24, height: 24 }}
              />
            </Tooltip>
          )}
        </Space>
      </div>
      <div style={bodyStyle}>
        {message.data ? (
          message.metadata?.isJson && highlightedHtml ? (
            searchTokens.length > 0 ? (
              <pre
                style={{
                  margin: 0,
                  fontSize: 12,
                  fontFamily: 'monospace',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                  lineHeight: 1.5,
                  color: token.colorText,
                  backgroundColor: token.colorFillQuaternary,
                  padding: 8,
                  borderRadius: 4,
                }}
              >
                {renderHighlightedText(displayContent)}
              </pre>
            ) : (
              <pre
                style={{
                  margin: 0,
                  fontSize: 12,
                  fontFamily: 'monospace',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                  lineHeight: 1.5,
                  backgroundColor: token.colorFillQuaternary,
                  padding: 8,
                  borderRadius: 4,
                }}
              >
                <code
                  className="hljs"
                  dangerouslySetInnerHTML={{
                    __html: shouldTruncate ? highlightJson(displayContent) : highlightedHtml,
                  }}
                />
              </pre>
            )
          ) : (
            <pre
              style={{
                margin: 0,
                fontSize: 12,
                fontFamily: 'monospace',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
                lineHeight: 1.5,
                color: token.colorText,
              }}
            >
              {searchTokens.length > 0 ? renderHighlightedText(displayContent) : displayContent}
            </pre>
          )
        ) : (
          <Text type="secondary" style={{ fontSize: 12, fontStyle: 'italic' }}>
            {t('trafficDetail.messages.emptyData')}
          </Text>
        )}
      </div>
    </div>
  );
}
