import { useMemo, useState, memo, useCallback } from 'react';
import { Typography, Tag, theme, Collapse, Tooltip, Button, Space, Modal, Progress } from 'antd';
import {
  RobotOutlined,
  ToolOutlined,
  CopyOutlined,
  CheckOutlined,
  ApiOutlined,
  BulbOutlined,
  WarningOutlined,
  SwapRightOutlined,
  DashboardOutlined,
  ClockCircleOutlined,
  InfoCircleOutlined,
  ExpandOutlined,
  FlagOutlined,
  CheckCircleOutlined,
  ThunderboltOutlined,
  CloseCircleOutlined,
  ExclamationCircleOutlined,
  CalendarOutlined,
  TagOutlined,
  SearchOutlined,
  CommentOutlined,
  GlobalOutlined,
} from '@ant-design/icons';
import hljs from 'highlight.js/lib/core';
import json from 'highlight.js/lib/languages/json';
import '../../../../styles/hljs-github-theme.css';
import { copyToClipboard } from '../../../../utils/clipboard';
import { useI18n, type TranslationKey } from '../../../../i18n';

hljs.registerLanguage('json', json);

const { Text, Paragraph } = Typography;

type Msg = Record<string, unknown>;

const formatCharCount = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
};

const formatMs = (ms: number | undefined): string => {
  if (ms === undefined || ms === null) return '-';
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms}ms`;
};

const formatTimestamp = (ts: number): string => {
  try {
    const d = ts > 1e12 ? new Date(ts) : new Date(ts * 1000);
    return d.toLocaleString();
  } catch {
    return String(ts);
  }
};

const MAX_HIGHLIGHT_SIZE = 50_000;
const COLLAPSIBLE_MAX_LINES = 10;
const INVISIBLE_CHARS_RE = /[\s\u200B-\u200D\u2060\u2063\uFEFF]/g;

const CopyButton = memo(({ text }: { text: string }) => {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const { token } = theme.useToken();
  const handleCopy = useCallback(async () => {
    const ok = await copyToClipboard(text);
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  }, [text]);
  return (
    <Tooltip title={copied ? t('trafficDetail.messages.copiedBang') : t('common.copy')}>
      <Button
        type="text"
        size="small"
        icon={copied ? <CheckOutlined style={{ color: token.colorSuccess }} /> : <CopyOutlined />}
        onClick={handleCopy}
        style={{ width: 24, height: 24 }}
      />
    </Tooltip>
  );
});

const JsonBlock = memo(({ data }: { data: string }) => {
  const { token } = theme.useToken();
  const highlighted = useMemo(() => {
    if (data.length > MAX_HIGHLIGHT_SIZE) return null;
    try {
      return hljs.highlight(data, { language: 'json' }).value;
    } catch {
      return null;
    }
  }, [data]);

  return (
    <div style={{ position: 'relative' }}>
      <div style={{ position: 'absolute', top: 4, right: 4, zIndex: 1 }}>
        <CopyButton text={data} />
      </div>
      <pre
        style={{
          margin: 0,
          padding: '8px 32px 8px 8px',
          fontSize: 12,
          fontFamily: 'monospace',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-all',
          lineHeight: 1.5,
          backgroundColor: token.colorFillQuaternary,
          borderRadius: 6,
          border: `1px solid ${token.colorBorderSecondary}`,
          maxHeight: 300,
          overflow: 'auto',
        }}
      >
        {highlighted
          ? <code className="hljs" dangerouslySetInnerHTML={{ __html: highlighted }} />
          : <code>{data}</code>
        }
      </pre>
    </div>
  );
});

const CollapsibleContent = memo(({ text, title }: { text: string; title?: string }) => {
  const { t } = useI18n();
  const [modalOpen, setModalOpen] = useState(false);
  const { token } = theme.useToken();
  const lines = useMemo(() => text.split('\n'), [text]);
  const needsTruncation = lines.length > COLLAPSIBLE_MAX_LINES;
  const previewText = needsTruncation ? lines.slice(0, COLLAPSIBLE_MAX_LINES).join('\n') : text;
  const modalTitle = title ?? t('trafficDetail.ai.fullContent');

  if (!text.trim()) {
    return <Text type="secondary" italic style={{ fontSize: 12 }}>{t('trafficDetail.ai.empty')}</Text>;
  }

  return (
    <>
      <div style={{ position: 'relative' }}>
        <div style={{ position: 'absolute', top: 0, right: 0, zIndex: 1 }}>
          <CopyButton text={text} />
        </div>
        <Paragraph
          style={{
            margin: 0,
            fontSize: 13,
            lineHeight: 1.7,
            whiteSpace: 'pre-wrap',
            wordBreak: 'break-word',
            paddingRight: 28,
            color: token.colorText,
          }}
        >
          {previewText}
          {needsTruncation && (
            <span style={{ color: token.colorTextSecondary }}>{'\n'}…</span>
          )}
        </Paragraph>
        {needsTruncation && (
          <Button
            type="link"
            size="small"
            icon={<ExpandOutlined />}
            onClick={() => setModalOpen(true)}
            style={{ padding: 0, fontSize: 11, height: 'auto', marginTop: 2 }}
          >
            {t('trafficDetail.ai.showAllLines', { lines: lines.length, chars: formatCharCount(text.length) })}
          </Button>
        )}
      </div>
      {needsTruncation && (
        <Modal
          title={
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', paddingRight: 24 }}>
              <span>{t('trafficDetail.ai.titleWithLines', { title: modalTitle, lines: lines.length })}</span>
              <CopyButton text={text} />
            </div>
          }
          open={modalOpen}
          onCancel={() => setModalOpen(false)}
          footer={null}
          width="90vw"
          centered
          styles={{
            body: { height: '80vh', overflow: 'auto', padding: 0 },
            header: { padding: '8px 16px', marginBottom: 0 },
          }}
        >
          <pre
            style={{
              margin: 0,
              padding: 12,
              fontSize: 12,
              fontFamily: 'monospace',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all',
              lineHeight: 1.6,
            }}
          >
            {text}
          </pre>
        </Modal>
      )}
    </>
  );
});

const formatToolArgs = (args: unknown): string => {
  if (typeof args === 'string') {
    try {
      return JSON.stringify(JSON.parse(args), null, 2);
    } catch {
      return args;
    }
  }
  if (args === null || args === undefined) return '';
  return JSON.stringify(args, null, 2);
};

const SectionCard = memo(({ icon, title, headerBg, extra, children }: {
  icon: React.ReactNode;
  title: string;
  headerBg: string;
  extra?: React.ReactNode;
  children: React.ReactNode;
}) => {
  const { token } = theme.useToken();
  return (
    <div
      style={{
        borderRadius: 8,
        border: `1px solid ${token.colorBorderSecondary}`,
        overflow: 'hidden',
        backgroundColor: token.colorBgContainer,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 6,
          padding: '6px 10px',
          backgroundColor: headerBg,
          borderBottom: `1px solid ${token.colorBorderSecondary}`,
        }}
      >
        <Space size={6}>
          {icon}
          <Text strong style={{ fontSize: 13 }}>{title}</Text>
        </Space>
        {extra}
      </div>
      <div style={{ padding: '6px 10px' }}>{children}</div>
    </div>
  );
});

const MetaRow = memo(({ label, value }: {
  label: string;
  value: React.ReactNode;
}) => {
  const { token } = theme.useToken();
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '4px 0',
        borderBottom: `1px solid ${token.colorFillQuaternary}`,
        gap: 8,
      }}
    >
      <Text type="secondary" style={{ fontSize: 12, flexShrink: 0 }}>{label}</Text>
      <div style={{ textAlign: 'right', minWidth: 0, overflow: 'hidden', fontSize: 12 }}>
        {value}
      </div>
    </div>
  );
});

const ToolCallCard = memo(({ toolCall, index }: { toolCall: Msg; index: number }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();

  const fn = toolCall.function as Msg | undefined;
  const name = (fn?.name as string) ?? (toolCall.tool_name as string) ?? 'unknown';
  const id = (toolCall.id as string) ?? '';
  const toolType = (toolCall.type as string) ?? '';
  const rawArgs = fn?.arguments ?? toolCall.arguments;
  const args = formatToolArgs(rawArgs);
  const requireLocal = toolCall.require_local_execution;

  return (
    <div
      style={{
        borderRadius: 8,
        border: `1px solid rgba(250,140,22,0.3)`,
        overflow: 'hidden',
        backgroundColor: token.colorBgContainer,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          padding: '5px 10px',
          backgroundColor: 'rgba(250,140,22,0.06)',
          borderBottom: `1px solid rgba(250,140,22,0.2)`,
          flexWrap: 'wrap',
        }}
      >
        <Text type="secondary" style={{ fontSize: 10, fontFamily: 'monospace' }}>#{index + 1}</Text>
        <ApiOutlined style={{ color: '#fa8c16', fontSize: 12 }} />
        <Text strong style={{ fontSize: 12, color: '#fa8c16' }}>{name}</Text>
        {toolType && toolType !== 'function' && (
          <Tag style={{ margin: 0, fontSize: 10, lineHeight: '16px', padding: '0 4px' }}>{toolType}</Tag>
        )}
        {!!requireLocal && (
          <Tag color="orange" style={{ margin: 0, fontSize: 10, lineHeight: '16px', padding: '0 4px' }}>{t('trafficDetail.sse.local')}</Tag>
        )}
        {id && (
          <Text type="secondary" style={{ fontSize: 10, fontFamily: 'monospace', opacity: 0.7, marginLeft: 'auto' }}>{id}</Text>
        )}
      </div>
      {args && args !== '{}' && (
        <div style={{ padding: '6px 10px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
            <SwapRightOutlined style={{ fontSize: 10, color: '#1677ff' }} />
            <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.ai.arguments')}</Text>
          </div>
          <JsonBlock data={args} />
        </div>
      )}
    </div>
  );
});

const TokenUsageSection = memo(({ usage }: { usage: Msg }) => {
  const { t } = useI18n();
  const promptTokens = (usage.prompt_tokens as number) ?? (usage.input_tokens as number) ?? 0;
  const completionTokens = (usage.completion_tokens as number) ?? (usage.output_tokens as number) ?? 0;
  const totalTokens = (usage.total_tokens as number) ?? (promptTokens + completionTokens);
  const cluster = usage.cluster as string | undefined;

  const promptDetails = usage.prompt_tokens_details as Msg | undefined ?? usage.input_tokens_details as Msg | undefined;
  const completionDetails = usage.completion_tokens_details as Msg | undefined ?? usage.output_tokens_details as Msg | undefined;

  const cachedTokens = (promptDetails?.cached_tokens as number | undefined)
    ?? (usage.cache_read_input_tokens as number | undefined);
  const audioInputTokens = promptDetails?.audio_tokens as number | undefined;
  const cacheCreationTokens = usage.cache_creation_input_tokens as number | undefined;

  const reasoningTokens = (completionDetails?.reasoning_tokens as number | undefined)
    ?? (usage.reasoning_tokens as number | undefined);
  const audioOutputTokens = completionDetails?.audio_tokens as number | undefined;
  const acceptedPredictionTokens = completionDetails?.accepted_prediction_tokens as number | undefined;
  const rejectedPredictionTokens = completionDetails?.rejected_prediction_tokens as number | undefined;

  const items: Array<{ label: string; value: number; color: string }> = [
    { label: t('trafficDetail.sse.tokenPrompt'), value: promptTokens, color: '#1677ff' },
    { label: t('trafficDetail.sse.tokenCompletion'), value: completionTokens, color: '#52c41a' },
  ];

  const addIf = (labelKey: TranslationKey, val: number | undefined, color: string) => {
    if (val && val > 0) items.push({ label: t(labelKey), value: val, color });
  };
  addIf('trafficDetail.sse.tokenCached', cachedTokens, '#13c2c2');
  addIf('trafficDetail.sse.tokenCacheCreation', cacheCreationTokens, '#fa8c16');
  addIf('trafficDetail.ai.reasoning', reasoningTokens, '#722ed1');
  addIf('trafficDetail.sse.tokenAudioIn', audioInputTokens, '#eb2f96');
  addIf('trafficDetail.sse.tokenAudioOut', audioOutputTokens, '#eb2f96');
  addIf('trafficDetail.sse.tokenAcceptedPrediction', acceptedPredictionTokens, '#389e0d');
  addIf('trafficDetail.sse.tokenRejectedPrediction', rejectedPredictionTokens, '#cf1322');

  const { token: antToken } = theme.useToken();

  return (
    <SectionCard
      icon={<DashboardOutlined style={{ color: '#fa8c16', fontSize: 13 }} />}
      title={t('trafficDetail.sse.tokenUsage')}
      headerBg="rgba(250,140,22,0.06)"
      extra={
        <Space size={6}>
          <Tag style={{ margin: 0, fontSize: 11, fontFamily: 'monospace' }}>Σ {totalTokens.toLocaleString()}</Tag>
          {cluster && <Tag style={{ margin: 0, fontSize: 10 }}>{cluster}</Tag>}
        </Space>
      }
    >
      {items.map(({ label, value, color }) => {
        const pct = totalTokens > 0 ? (value / totalTokens) * 100 : 0;
        return (
          <div
            key={label}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '3px 0',
              borderBottom: `1px solid ${antToken.colorFillQuaternary}`,
            }}
          >
            <Text type="secondary" style={{ fontSize: 11, width: 130, flexShrink: 0 }}>{label}</Text>
            <Progress percent={Math.min(pct, 100)} showInfo={false} strokeColor={color} size="small" style={{ flex: 1, margin: 0 }} />
            <Text style={{ fontSize: 11, fontFamily: 'monospace', width: 70, textAlign: 'right', flexShrink: 0 }}>{value.toLocaleString()}</Text>
          </div>
        );
      })}
    </SectionCard>
  );
});

const TimingSection = memo(({ timing }: { timing: Msg }) => {
  const { t } = useI18n();
  const serverTime = (timing.server_processing_time as number) ?? (timing.gateway_server_processing_time as number) ?? 0;
  const firstToken = (timing.platform_first_token_timing as number) ?? (timing.first_sse_event_time as number) ?? 0;

  const rows: Array<{ label: string; value: number | undefined; color: string }> = [
    { label: t('trafficDetail.sse.timingServerProcessing'), value: timing.server_processing_time as number | undefined, color: '#1677ff' },
    { label: t('trafficDetail.sse.timingFirstToken'), value: timing.platform_first_token_timing as number | undefined, color: '#52c41a' },
    { label: t('trafficDetail.sse.timingFirstSseEvent'), value: timing.first_sse_event_time as number | undefined, color: '#13c2c2' },
    { label: t('trafficDetail.sse.timingGatewayPreprocess'), value: timing.gateway_preprocess_timing as number | undefined, color: '#fa8c16' },
    { label: t('trafficDetail.sse.timingPreprocess'), value: timing.preprocess_timing as number | undefined, color: '#722ed1' },
    { label: t('trafficDetail.sse.timingMiddleware'), value: timing.middleware_processing_time as number | undefined, color: '#eb2f96' },
    { label: t('trafficDetail.sse.timingQueue'), value: timing.queue_timing as number | undefined, color: '#8c8c8c' },
  ].filter(r => r.value !== undefined && r.value !== null && (r.value as number) > 0);

  const { token: antToken } = theme.useToken();
  const providerModel = timing.provider_model_name as string | undefined;
  const configName = timing.config_name as string | undefined;

  return (
    <SectionCard
      icon={<ClockCircleOutlined style={{ color: '#13c2c2', fontSize: 13 }} />}
      title={t('trafficDetail.sse.timing')}
      headerBg="rgba(19,194,194,0.06)"
      extra={
        <Space size={8}>
          {serverTime > 0 && <Tag color="blue" style={{ margin: 0, fontSize: 11 }}>{t('trafficDetail.sse.total', { value: formatMs(serverTime) })}</Tag>}
          {firstToken > 0 && <Tag color="green" style={{ margin: 0, fontSize: 11 }}>TTFT: {formatMs(firstToken)}</Tag>}
        </Space>
      }
    >
      {rows.map(({ label, value, color }) => {
        const pct = serverTime > 0 ? ((value ?? 0) / serverTime) * 100 : 0;
        return (
          <div
            key={label}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '3px 0',
              borderBottom: `1px solid ${antToken.colorFillQuaternary}`,
            }}
          >
            <Text type="secondary" style={{ fontSize: 11, width: 120, flexShrink: 0 }}>{label}</Text>
            <Progress percent={Math.min(pct, 100)} showInfo={false} strokeColor={color} size="small" style={{ flex: 1, margin: 0 }} />
            <Text style={{ fontSize: 11, fontFamily: 'monospace', width: 60, textAlign: 'right', flexShrink: 0 }}>{formatMs(value)}</Text>
          </div>
        );
      })}
      {providerModel && (
        <div style={{ padding: '4px 0' }}>
          <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.provider')} </Text>
          <Text style={{ fontSize: 11, fontFamily: 'monospace' }}>{providerModel}</Text>
        </div>
      )}
      {configName && (
        <div style={{ padding: '2px 0' }}>
          <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.config')} </Text>
          <Tag style={{ margin: 0, fontSize: 10 }}>{configName}</Tag>
        </div>
      )}
    </SectionCard>
  );
});

interface ChoiceData {
  index: number;
  finishReason: string | null;
  content: string;
  reasoningContent: string;
  refusal: string;
  toolCalls: Msg[];
  functionCall: Msg | null;
  annotations: Msg[];
  logprobs: Msg | null;
  extra: Msg;
}

interface OpenAiData {
  type: 'openai';
  mode: 'chat.completions' | 'responses';
  model?: string;
  id?: string;
  object?: string;
  created?: number;
  systemFingerprint?: string;
  serviceTier?: string;
  choices: ChoiceData[];
  usage: Msg | null;
  error: Msg | null;
  status?: string;
  incompleteDetails: Msg | null;
  outputText?: string;
  reasoning: Msg | null;
  extra: Msg;
}

interface TraeData {
  type: 'trae';
  metadata: Msg | null;
  timingCost: Msg | null;
  content: string;
  thought: string;
  reasoningContent: string;
  toolCalls: Msg[];
  usage: Msg | null;
  turnCompletion: Msg | null;
  task: Msg | null;
  finishReason?: string;
  done: Msg | null;
  agentName?: string;
  agentType?: string;
  requiredContexts: string[];
  progressNotices: string[];
}

interface DouBaoData {
  type: 'doubao';
  agentName: string;
  modelType: string;
  isDeepThinking: boolean;
  conversationId: string;
  messageId: string;
  thinkingTitle: string;
  thinkingFinishTitle: string;
  thinkingSteps: string[];
  searchQueries: string[];
  content: string;
  brief: string;
  finishType: string;
  replyEnds: Msg[];
  meta: Msg | null;
}

type ParsedData = OpenAiData | TraeData | DouBaoData;

const parseAssemblyBody = (body: string, mode: 'openai' | 'trae' | 'doubao'): ParsedData | null => {
  try {
    const obj = JSON.parse(body) as Msg;
    if (mode === 'openai') {
      return parseOpenAiBody(obj);
    }
    if (mode === 'doubao') {
      return parseDouBaoBody(obj);
    }
    return parseTraeBody(obj);
  } catch {
    return null;
  }
};

const parseOpenAiBody = (obj: Msg): OpenAiData => {
  const objectType = typeof obj.object === 'string' ? obj.object : undefined;
  const isResponses = objectType === 'response' || objectType?.startsWith('response');

  if (isResponses) {
    return parseResponsesBody(obj);
  }
  return parseChatCompletionsBody(obj);
};

const parseChatCompletionsBody = (obj: Msg): OpenAiData => {
  const choices: ChoiceData[] = Array.isArray(obj.choices)
    ? (obj.choices as Msg[]).map((c) => {
        const msg = (c.message as Msg) ?? {};
        const content = typeof msg.content === 'string' ? msg.content : '';
        const reasoningContent = typeof msg.reasoning_content === 'string' ? msg.reasoning_content : '';
        const refusal = typeof msg.refusal === 'string' ? msg.refusal : '';
        const toolCalls = Array.isArray(msg.tool_calls) ? (msg.tool_calls as Msg[]) : [];
        const functionCall = msg.function_call as Msg | null ?? null;
        const annotations = Array.isArray(msg.annotations) ? (msg.annotations as Msg[]) : [];
        const logprobs = c.logprobs as Msg | null ?? null;

        const extraKeys = new Set(['index', 'message', 'finish_reason', 'logprobs']);
        const extra: Msg = {};
        for (const [k, v] of Object.entries(c)) {
          if (!extraKeys.has(k) && v !== undefined) extra[k] = v;
        }

        return {
          index: typeof c.index === 'number' ? c.index : 0,
          finishReason: typeof c.finish_reason === 'string' ? c.finish_reason : null,
          content,
          reasoningContent,
          refusal,
          toolCalls,
          functionCall,
          annotations,
          logprobs,
          extra,
        };
      })
    : [];

  const topExclude = new Set(['choices', 'object', 'usage', 'id', 'model', 'created', 'system_fingerprint', 'service_tier']);
  const extra: Msg = {};
  for (const [k, v] of Object.entries(obj)) {
    if (!topExclude.has(k) && v !== undefined) extra[k] = v;
  }

  return {
    type: 'openai',
    mode: 'chat.completions',
    model: typeof obj.model === 'string' ? obj.model : undefined,
    id: typeof obj.id === 'string' ? obj.id : undefined,
    object: typeof obj.object === 'string' ? obj.object : undefined,
    created: typeof obj.created === 'number' ? obj.created : undefined,
    systemFingerprint: typeof obj.system_fingerprint === 'string' ? obj.system_fingerprint : undefined,
    serviceTier: typeof obj.service_tier === 'string' ? obj.service_tier : undefined,
    choices,
    usage: obj.usage as Msg | null ?? null,
    error: null,
    status: undefined,
    incompleteDetails: null,
    reasoning: null,
    extra,
  };
};

const parseResponsesBody = (obj: Msg): OpenAiData => {
  const choices: ChoiceData[] = [];

  if (Array.isArray(obj.output)) {
    for (const [idx, item] of (obj.output as Msg[]).entries()) {
      if (!item || typeof item !== 'object') continue;
      const role = typeof item.role === 'string' ? item.role : 'assistant';
      let content = '';
      const toolCalls: Msg[] = [];
      const annotations: Msg[] = [];

      if (Array.isArray(item.content)) {
        for (const part of item.content as Msg[]) {
          if (!part || typeof part !== 'object') continue;
          const partType = typeof part.type === 'string' ? part.type : '';
          if (partType === 'output_text' || partType === 'text') {
            content += typeof part.text === 'string' ? part.text : '';
            if (Array.isArray(part.annotations)) {
              annotations.push(...(part.annotations as Msg[]));
            }
          } else if (partType === 'function_call' || partType === 'tool_use') {
            toolCalls.push(part);
          }
        }
      }

      if (typeof item.type === 'string' && item.type === 'function_call') {
        toolCalls.push(item);
        continue;
      }

      choices.push({
        index: idx,
        finishReason: typeof item.status === 'string' ? item.status : null,
        content,
        reasoningContent: '',
        refusal: '',
        toolCalls,
        functionCall: null,
        annotations,
        logprobs: null,
        extra: { role },
      });
    }
  }

  if (choices.length === 0 && typeof obj.output_text === 'string' && obj.output_text) {
    choices.push({
      index: 0,
      finishReason: typeof obj.status === 'string' ? obj.status : null,
      content: obj.output_text,
      reasoningContent: '',
      refusal: '',
      toolCalls: [],
      functionCall: null,
      annotations: [],
      logprobs: null,
      extra: {},
    });
  }

  const topExclude = new Set([
    'output', 'output_text', 'object', 'usage', 'id', 'model', 'created_at', 'created',
    'service_tier', 'error', 'status', 'incomplete_details', 'reasoning',
  ]);
  const extra: Msg = {};
  for (const [k, v] of Object.entries(obj)) {
    if (!topExclude.has(k) && v !== undefined && v !== null) extra[k] = v;
  }

  const created = typeof obj.created_at === 'number' ? obj.created_at
    : typeof obj.created === 'number' ? obj.created : undefined;

  return {
    type: 'openai',
    mode: 'responses',
    model: typeof obj.model === 'string' ? obj.model : undefined,
    id: typeof obj.id === 'string' ? obj.id : undefined,
    object: typeof obj.object === 'string' ? obj.object : undefined,
    created,
    systemFingerprint: undefined,
    serviceTier: typeof obj.service_tier === 'string' ? obj.service_tier : undefined,
    choices,
    usage: obj.usage as Msg | null ?? null,
    error: obj.error as Msg | null ?? null,
    status: typeof obj.status === 'string' ? obj.status : undefined,
    incompleteDetails: obj.incomplete_details as Msg | null ?? null,
    outputText: typeof obj.output_text === 'string' ? obj.output_text : undefined,
    reasoning: obj.reasoning as Msg | null ?? null,
    extra,
  };
};

const parseTraeBody = (obj: Msg): TraeData => {
  const msg = (obj.message as Msg) ?? {};
  return {
    type: 'trae',
    metadata: obj.metadata as Msg | null ?? null,
    timingCost: obj.timing_cost as Msg | null ?? null,
    content: typeof msg.content === 'string' ? msg.content : '',
    thought: typeof msg.thought === 'string' ? msg.thought : '',
    reasoningContent: typeof msg.reasoning_content === 'string' ? msg.reasoning_content : '',
    toolCalls: Array.isArray(msg.tool_calls) ? (msg.tool_calls as Msg[]) : [],
    usage: obj.usage as Msg | null ?? null,
    turnCompletion: obj.turn_completion as Msg | null ?? null,
    task: obj.task as Msg | null ?? null,
    finishReason: typeof obj.finish_reason === 'string' ? obj.finish_reason : undefined,
    done: obj.done as Msg | null ?? null,
    agentName: typeof msg.agent_name === 'string' ? msg.agent_name : undefined,
    agentType: typeof msg.agent_type === 'string' ? msg.agent_type : undefined,
    requiredContexts: Array.isArray(obj.required_contexts) ? (obj.required_contexts as string[]) : [],
    progressNotices: Array.isArray(obj.progress_notices) ? (obj.progress_notices as string[]) : [],
  };
};

const parseDouBaoBody = (obj: Msg): DouBaoData => {
  const msg = (obj.message as Msg) ?? {};
  const thinking = (obj.thinking as Msg) ?? {};
  return {
    type: 'doubao',
    agentName: typeof obj.agent_name === 'string' ? obj.agent_name : '',
    modelType: typeof obj.model_type === 'string' ? obj.model_type : '',
    isDeepThinking: obj.is_deep_thinking === true,
    conversationId: typeof obj.conversation_id === 'string' ? obj.conversation_id : '',
    messageId: typeof obj.message_id === 'string' ? obj.message_id : '',
    thinkingTitle: typeof thinking.title === 'string' ? thinking.title : '',
    thinkingFinishTitle: typeof thinking.finish_title === 'string' ? thinking.finish_title : '',
    thinkingSteps: Array.isArray(thinking.steps) ? (thinking.steps as string[]) : [],
    searchQueries: Array.isArray(obj.search_queries) ? (obj.search_queries as string[]) : [],
    content: typeof msg.content === 'string' ? msg.content : '',
    brief: typeof obj.brief === 'string' ? obj.brief : '',
    finishType: typeof obj.finish_type === 'string' ? obj.finish_type : '',
    replyEnds: Array.isArray(obj.reply_ends) ? (obj.reply_ends as Msg[]) : [],
    meta: obj.meta as Msg | null ?? null,
  };
};

const ErrorBlock = memo(({ error }: { error: Msg }) => {
  const { t } = useI18n();
  return (
  <div
    style={{
      padding: '6px 8px',
      backgroundColor: 'rgba(255,77,79,0.06)',
      borderRadius: 6,
      borderLeft: '3px solid #ff4d4f',
    }}
  >
    <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
      <CloseCircleOutlined style={{ fontSize: 11, color: '#ff4d4f' }} />
      <Text strong style={{ fontSize: 11, color: '#ff4d4f' }}>{t('common.error')}</Text>
    </div>
    <JsonBlock data={JSON.stringify(error, null, 2)} />
  </div>
  );
});

const AnnotationsBlock = memo(({ annotations }: { annotations: Msg[] }) => {
  const { t } = useI18n();
  if (annotations.length === 0) return null;
  return (
    <div style={{ padding: '4px 0' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
        <TagOutlined style={{ fontSize: 11, color: '#8c8c8c' }} />
        <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.annotations', { count: annotations.length })}</Text>
      </div>
      <JsonBlock data={JSON.stringify(annotations, null, 2)} />
    </div>
  );
});

const LogprobsBlock = memo(({ logprobs }: { logprobs: Msg }) => (
  <Collapse
    size="small"
    items={[{
      key: 'logprobs',
      label: <Text type="secondary" style={{ fontSize: 11 }}>Logprobs</Text>,
      children: <JsonBlock data={JSON.stringify(logprobs, null, 2)} />,
    }]}
    style={{ backgroundColor: 'transparent' }}
  />
));

const OpenAiResponsePanel = memo(({ data, rawBody }: { data: OpenAiData; rawBody: string }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const hasModelInfo = data.model || data.id || data.created || data.systemFingerprint || data.serviceTier;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '8px 4px' }}>
      {data.error && <ErrorBlock error={data.error} />}

      {data.incompleteDetails && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0' }}>
          <ExclamationCircleOutlined style={{ color: '#fa8c16', fontSize: 12 }} />
          <Text type="secondary" style={{ fontSize: 12 }}>{t('trafficDetail.sse.incomplete')}</Text>
          <Text style={{ fontSize: 12 }}>{JSON.stringify(data.incompleteDetails)}</Text>
        </div>
      )}

      {hasModelInfo && (
        <SectionCard
          icon={<RobotOutlined style={{ color: '#1677ff', fontSize: 13 }} />}
          title={data.mode === 'responses' ? t('trafficDetail.sse.responseInfo') : t('trafficDetail.sse.modelInfo')}
          headerBg="rgba(22,119,255,0.06)"
          extra={
            <Space size={4}>
              {data.status && (
                <Tag
                  color={data.status === 'completed' ? 'green' : data.status === 'failed' ? 'red' : 'default'}
                  style={{ margin: 0, fontSize: 10 }}
                >
                  {data.status}
                </Tag>
              )}
              {data.object && data.object !== 'chat.completion' && (
                <Tag style={{ margin: 0, fontSize: 10 }}>{data.object}</Tag>
              )}
            </Space>
          }
        >
          {data.model && <MetaRow label={t('trafficDetail.ai.model')} value={<Text strong style={{ fontFamily: 'monospace', fontSize: 12 }}>{data.model}</Text>} />}
          {data.id && <MetaRow label="ID" value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{data.id}</Text>} />}
          {data.created !== undefined && (
            <MetaRow
              label={t('trafficDetail.sse.created')}
              value={
                <Space size={4}>
                  <CalendarOutlined style={{ fontSize: 10, color: '#8c8c8c' }} />
                  <Text style={{ fontSize: 11, fontFamily: 'monospace' }}>{formatTimestamp(data.created)}</Text>
                </Space>
              }
            />
          )}
          {data.systemFingerprint && <MetaRow label={t('trafficDetail.sse.fingerprint')} value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{data.systemFingerprint}</Text>} />}
          {data.serviceTier && <MetaRow label={t('trafficDetail.ai.paramLabel.serviceTier')} value={<Tag style={{ margin: 0, fontSize: 10 }}>{data.serviceTier}</Tag>} />}
        </SectionCard>
      )}

      {data.usage && <TokenUsageSection usage={data.usage} />}

      {data.reasoning && typeof data.reasoning === 'object' && Object.keys(data.reasoning).length > 0 && (
        (() => {
          const r = data.reasoning as Msg;
          const effort = r.effort as string | undefined;
          const summary = r.summary as string | undefined;
          if (!effort && !summary) return null;
          return (
            <div
              style={{
                padding: '6px 8px',
                backgroundColor: 'rgba(114,46,209,0.06)',
                borderRadius: 6,
                borderLeft: '3px solid #722ed1',
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
                <BulbOutlined style={{ fontSize: 11, color: '#722ed1' }} />
                <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.reasoningConfig')}</Text>
              </div>
              {effort && (
                <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                  <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.effort')}</Text>
                  <Tag style={{ margin: 0, fontSize: 10 }}>{effort}</Tag>
                </div>
              )}
              {summary && (
                <div style={{ marginTop: 4 }}>
                  <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.summary')}</Text>
                  <Paragraph style={{ margin: 0, fontSize: 12, marginTop: 2 }}>{summary}</Paragraph>
                </div>
              )}
            </div>
          );
        })()
      )}

      {data.choices.map((choice) => (
        <ChoicePanel key={choice.index} choice={choice} totalChoices={data.choices.length} isResponses={data.mode === 'responses'} />
      ))}

      <Collapse
        size="small"
        items={[{
          key: 'raw',
          label: <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.rawJson')}</Text>,
          children: <JsonBlock data={rawBody} />,
        }]}
        style={{ backgroundColor: 'transparent', borderColor: token.colorBorderSecondary }}
      />
    </div>
  );
});

const ChoicePanel = memo(({ choice, totalChoices, isResponses }: {
  choice: ChoiceData;
  totalChoices: number;
  isResponses?: boolean;
}) => {
  const { t } = useI18n();
  const hasContent = choice.content.trim().length > 0;
  const hasReasoning = choice.reasoningContent.replace(INVISIBLE_CHARS_RE, '').length > 0;
  const hasRefusal = choice.refusal.trim().length > 0;
  const hasToolCalls = choice.toolCalls.length > 0;
  const hasFunctionCall = choice.functionCall !== null;
  const hasAnnotations = choice.annotations.length > 0;
  const hasLogprobs = choice.logprobs !== null;

  const finishReason = choice.finishReason;
  const finishColor = finishReason === 'stop' || finishReason === 'completed' ? 'green'
    : finishReason === 'tool_calls' ? 'orange'
    : finishReason === 'content_filter' ? 'red'
    : finishReason === 'length' ? 'gold'
    : finishReason === 'failed' || finishReason === 'cancelled' ? 'red'
    : 'default';

  const title = totalChoices > 1
    ? (isResponses
      ? t('trafficDetail.sse.outputN', { index: choice.index })
      : t('trafficDetail.sse.choiceN', { index: choice.index }))
    : t('trafficDetail.sse.assistantOutput');

  return (
    <SectionCard
      icon={<RobotOutlined style={{ color: '#52c41a', fontSize: 13 }} />}
      title={title}
      headerBg="rgba(82,196,26,0.06)"
      extra={
        <Space size={6}>
          {finishReason && (
            <Tag color={finishColor} style={{ margin: 0, fontSize: 10 }}>
              {finishReason}
            </Tag>
          )}
          {hasContent && (
            <Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>
              {formatCharCount(choice.content.length)}
            </Tag>
          )}
        </Space>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {hasReasoning && (
          <ReasoningBlock text={choice.reasoningContent} />
        )}

        {hasContent && (
          <CollapsibleContent text={choice.content} title={t('trafficDetail.sse.assistantContent')} />
        )}

        {hasRefusal && (
          <RefusalBlock text={choice.refusal} />
        )}

        {hasFunctionCall && (
          <ToolCallCard toolCall={{ function: choice.functionCall } as Msg} index={0} />
        )}

        {hasToolCalls && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
              <ToolOutlined style={{ fontSize: 12, color: '#fa8c16' }} />
              <Text strong style={{ fontSize: 12, color: '#fa8c16' }}>
                {t('trafficDetail.sse.toolCallsCount', { count: choice.toolCalls.length })}
              </Text>
            </div>
            {choice.toolCalls.map((tc, i) => (
              <ToolCallCard key={(tc.id as string) ?? i} toolCall={tc} index={i} />
            ))}
          </div>
        )}

        {hasAnnotations && <AnnotationsBlock annotations={choice.annotations} />}

        {hasLogprobs && <LogprobsBlock logprobs={choice.logprobs!} />}

        {!hasContent && !hasReasoning && !hasRefusal && !hasToolCalls && !hasFunctionCall && (
          <Text type="secondary" italic style={{ fontSize: 12 }}>{t('trafficDetail.sse.emptyResponse')}</Text>
        )}
      </div>
    </SectionCard>
  );
});

const ReasoningBlock = memo(({ text }: { text: string }) => {
  const { t } = useI18n();
  return (
  <div
    style={{
      padding: '6px 8px',
      backgroundColor: 'rgba(114,46,209,0.06)',
      borderRadius: 6,
      borderLeft: '3px solid #722ed1',
    }}
  >
    <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
      <BulbOutlined style={{ fontSize: 11, color: '#722ed1' }} />
      <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.ai.reasoning')}</Text>
      <Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>
        {formatCharCount(text.length)}
      </Tag>
    </div>
    <CollapsibleContent text={text} title={t('trafficDetail.sse.reasoningContent')} />
  </div>
  );
});

const RefusalBlock = memo(({ text }: { text: string }) => {
  const { t } = useI18n();
  return (
  <div
    style={{
      padding: '6px 8px',
      backgroundColor: 'rgba(255,77,79,0.06)',
      borderRadius: 6,
      borderLeft: '3px solid #ff4d4f',
    }}
  >
    <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
      <WarningOutlined style={{ fontSize: 11, color: '#ff4d4f' }} />
      <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.ai.refusal')}</Text>
    </div>
    <Text style={{ fontSize: 12 }}>{text}</Text>
  </div>
  );
});

const TraeResponsePanel = memo(({ data, rawBody }: { data: TraeData; rawBody: string }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const model = typeof (data.metadata as Msg)?.model === 'string' ? (data.metadata as Msg).model as string : undefined;
  const sessionId = typeof (data.metadata as Msg)?.session_id === 'string' ? (data.metadata as Msg).session_id as string : undefined;
  const metaId = typeof (data.metadata as Msg)?.id === 'string' ? (data.metadata as Msg).id as string : undefined;
  const metaConfig = (data.metadata as Msg)?.config;
  const hasContent = data.content.trim().length > 0;
  const hasThought = data.thought.trim().length > 0;
  const hasReasoning = data.reasoningContent.replace(INVISIBLE_CHARS_RE, '').length > 0;
  const hasToolCalls = data.toolCalls.length > 0;
  const taskCompletion = data.turnCompletion?.task_completion;

  const finishReason = data.finishReason;
  const finishColor = finishReason === 'stop' ? 'green'
    : finishReason === 'tool_calls' ? 'orange'
    : finishReason === 'length' ? 'gold'
    : finishReason ? 'default' : undefined;

  const doneWith = data.done?.done_with as string | undefined;
  const doneMessage = data.done?.message as string | undefined;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '8px 4px' }}>
      {data.metadata && (
        <SectionCard
          icon={<RobotOutlined style={{ color: '#1677ff', fontSize: 13 }} />}
          title={t('trafficDetail.sse.modelInfo')}
          headerBg="rgba(22,119,255,0.06)"
          extra={data.agentName ? <Tag style={{ margin: 0, fontSize: 10 }}>{data.agentName}</Tag> : undefined}
        >
          {model && <MetaRow label={t('trafficDetail.ai.model')} value={<Text strong style={{ fontFamily: 'monospace', fontSize: 12 }}>{model}</Text>} />}
          {metaId && <MetaRow label="ID" value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{metaId}</Text>} />}
          {sessionId && <MetaRow label={t('trafficDetail.sse.session')} value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{sessionId}</Text>} />}
          {data.agentType && <MetaRow label={t('trafficDetail.sse.agentType')} value={<Tag style={{ margin: 0, fontSize: 10 }}>{data.agentType}</Tag>} />}
          {typeof metaConfig === 'string' && metaConfig && (
            <MetaRow label={t('trafficDetail.sse.configLabel')} value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{metaConfig}</Text>} />
          )}
        </SectionCard>
      )}

      {data.timingCost && <TimingSection timing={data.timingCost} />}

      {data.usage && <TokenUsageSection usage={data.usage} />}

      {hasReasoning && <ReasoningBlock text={data.reasoningContent} />}

      {hasThought && (
        <SectionCard
          icon={<BulbOutlined style={{ color: '#722ed1', fontSize: 13 }} />}
          title={t('trafficDetail.sse.thought')}
          headerBg="rgba(114,46,209,0.06)"
          extra={<Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>{formatCharCount(data.thought.length)}</Tag>}
        >
          <CollapsibleContent text={data.thought} title={t('trafficDetail.sse.thoughtContent')} />
        </SectionCard>
      )}

      {hasContent && (
        <SectionCard
          icon={<RobotOutlined style={{ color: '#52c41a', fontSize: 13 }} />}
          title={t('trafficDetail.sse.assistantOutput')}
          headerBg="rgba(82,196,26,0.06)"
          extra={
            <Space size={6}>
              {finishColor && finishReason && (
                <Tag color={finishColor} style={{ margin: 0, fontSize: 10 }}>{finishReason}</Tag>
              )}
              <Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>
                {formatCharCount(data.content.length)}
              </Tag>
            </Space>
          }
        >
          <CollapsibleContent text={data.content} title={t('trafficDetail.sse.assistantContent')} />
        </SectionCard>
      )}

      {hasToolCalls && (
        <SectionCard
          icon={<ToolOutlined style={{ color: '#fa8c16', fontSize: 13 }} />}
          title={t('trafficDetail.sse.toolCalls')}
          headerBg="rgba(250,140,22,0.06)"
          extra={<Tag color="orange" style={{ margin: 0, fontSize: 11 }}>{data.toolCalls.length}</Tag>}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {data.toolCalls.map((tc, i) => (
              <ToolCallCard key={(tc.id as string) ?? i} toolCall={tc} index={i} />
            ))}
          </div>
        </SectionCard>
      )}

      {data.turnCompletion && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0' }}>
          {taskCompletion
            ? <CheckCircleOutlined style={{ color: '#52c41a', fontSize: 12 }} />
            : <FlagOutlined style={{ color: '#8c8c8c', fontSize: 12 }} />
          }
          <Text type="secondary" style={{ fontSize: 12 }}>{t('trafficDetail.sse.turnCompletion')}</Text>
          <Tag color={taskCompletion ? 'green' : 'default'} style={{ margin: 0, fontSize: 11 }}>
            {taskCompletion ? t('trafficDetail.sse.taskComplete') : t('trafficDetail.sse.pending')}
          </Tag>
        </div>
      )}

      {data.done && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0' }}>
          <CheckCircleOutlined style={{ color: doneWith === 'end' ? '#52c41a' : '#8c8c8c', fontSize: 12 }} />
          <Text type="secondary" style={{ fontSize: 12 }}>{t('trafficDetail.sse.done')}</Text>
          {doneWith && <Tag color={doneWith === 'end' ? 'green' : 'default'} style={{ margin: 0, fontSize: 11 }}>{doneWith}</Tag>}
          {doneMessage && <Text type="secondary" style={{ fontSize: 11 }}>{doneMessage}</Text>}
        </div>
      )}

      {data.requiredContexts.length > 0 && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0', flexWrap: 'wrap' }}>
          <InfoCircleOutlined style={{ color: '#8c8c8c', fontSize: 12 }} />
          <Text type="secondary" style={{ fontSize: 12 }}>{t('trafficDetail.sse.requiredContexts')}</Text>
          {data.requiredContexts.map((ctx, i) => (
            <Tag key={i} style={{ margin: 0, fontSize: 11 }}>{ctx}</Tag>
          ))}
        </div>
      )}

      {data.progressNotices.length > 0 && (
        <Collapse
          size="small"
          items={[{
            key: 'progress',
            label: (
              <Space size={6}>
                <ThunderboltOutlined style={{ color: '#8c8c8c' }} />
                <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.progressNotices', { count: data.progressNotices.length })}</Text>
              </Space>
            ),
            children: (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                {data.progressNotices.map((n, i) => (
                  <Text key={i} type="secondary" style={{ fontSize: 11, fontFamily: 'monospace' }}>{n}</Text>
                ))}
              </div>
            ),
          }]}
          style={{ backgroundColor: 'transparent' }}
        />
      )}

      <Collapse
        size="small"
        items={[{
          key: 'raw',
          label: <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.rawJson')}</Text>,
          children: <JsonBlock data={rawBody} />,
        }]}
        style={{ backgroundColor: 'transparent', borderColor: token.colorBorderSecondary }}
      />
    </div>
  );
});

const DouBaoResponsePanel = memo(({ data, rawBody }: { data: DouBaoData; rawBody: string }) => {
  const { t } = useI18n();
  const { token } = theme.useToken();
  const hasContent = data.content.trim().length > 0;
  const hasThinking = data.thinkingSteps.length > 0;
  const hasSearch = data.searchQueries.length > 0;
  const hasBrief = data.brief.trim().length > 0;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '8px 4px' }}>
      <SectionCard
        icon={<RobotOutlined style={{ color: '#1677ff', fontSize: 13 }} />}
        title={t('trafficDetail.sse.modelInfo')}
        headerBg="rgba(22,119,255,0.06)"
        extra={data.agentName ? <Tag style={{ margin: 0, fontSize: 10 }}>{data.agentName}</Tag> : undefined}
      >
        {data.modelType && <MetaRow label={t('trafficDetail.sse.modelType')} value={<Text strong style={{ fontFamily: 'monospace', fontSize: 12 }}>{data.modelType}</Text>} />}
        {data.conversationId && <MetaRow label={t('trafficDetail.ai.conversation')} value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{data.conversationId}</Text>} />}
        {data.messageId && <MetaRow label={t('trafficDetail.sse.messageId')} value={<Text style={{ fontFamily: 'monospace', fontSize: 11, opacity: 0.8 }}>{data.messageId}</Text>} />}
        <MetaRow label={t('trafficDetail.sse.deepThinking')} value={
          <Tag color={data.isDeepThinking ? 'purple' : 'default'} style={{ margin: 0, fontSize: 10 }}>
            {data.isDeepThinking ? t('common.enabled') : t('common.disabled')}
          </Tag>
        } />
      </SectionCard>

      {hasThinking && (
        <SectionCard
          icon={<BulbOutlined style={{ color: '#722ed1', fontSize: 13 }} />}
          title={data.thinkingFinishTitle || data.thinkingTitle || t('trafficDetail.sse.thinking')}
          headerBg="rgba(114,46,209,0.06)"
          extra={<Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>{t('trafficDetail.sse.steps', { count: data.thinkingSteps.length })}</Tag>}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            {data.thinkingSteps.map((step, i) => (
              <div key={i} style={{
                padding: '4px 8px',
                backgroundColor: token.colorFillQuaternary,
                borderRadius: 4,
                fontSize: 12,
                fontFamily: 'monospace',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
              }}>
                {step}
              </div>
            ))}
          </div>
        </SectionCard>
      )}

      {hasSearch && (
        <SectionCard
          icon={<SearchOutlined style={{ color: '#13c2c2', fontSize: 13 }} />}
          title={t('trafficDetail.sse.searchQueries')}
          headerBg="rgba(19,194,194,0.06)"
          extra={<Tag color="cyan" style={{ margin: 0, fontSize: 10 }}>{data.searchQueries.length}</Tag>}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            {data.searchQueries.map((q, i) => (
              <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <GlobalOutlined style={{ fontSize: 11, color: '#8c8c8c', flexShrink: 0 }} />
                <Text style={{ fontSize: 12 }}>{q}</Text>
              </div>
            ))}
          </div>
        </SectionCard>
      )}

      {hasContent && (
        <SectionCard
          icon={<CommentOutlined style={{ color: '#52c41a', fontSize: 13 }} />}
          title={t('trafficDetail.sse.assistantOutput')}
          headerBg="rgba(82,196,26,0.06)"
          extra={
            <Space size={6}>
              {data.finishType && (
                <Tag color={data.finishType === 'msg_finish' ? 'green' : 'default'} style={{ margin: 0, fontSize: 10 }}>{data.finishType}</Tag>
              )}
              <Tag style={{ margin: 0, fontSize: 10, fontFamily: 'monospace' }}>
                {formatCharCount(data.content.length)}
              </Tag>
            </Space>
          }
        >
          <CollapsibleContent text={data.content} title={t('trafficDetail.sse.assistantContent')} />
        </SectionCard>
      )}

      {hasBrief && !hasContent && (
        <SectionCard
          icon={<CommentOutlined style={{ color: '#52c41a', fontSize: 13 }} />}
          title={t('trafficDetail.sse.brief')}
          headerBg="rgba(82,196,26,0.06)"
        >
          <Text style={{ fontSize: 12 }}>{data.brief}</Text>
        </SectionCard>
      )}

      {data.finishType && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0' }}>
          <CheckCircleOutlined style={{ color: data.finishType === 'msg_finish' ? '#52c41a' : '#8c8c8c', fontSize: 12 }} />
          <Text type="secondary" style={{ fontSize: 12 }}>{t('trafficDetail.sse.finish')}</Text>
          <Tag color={data.finishType === 'msg_finish' ? 'green' : 'default'} style={{ margin: 0, fontSize: 11 }}>{data.finishType}</Tag>
        </div>
      )}

      <Collapse
        size="small"
        items={[{
          key: 'raw',
          label: <Text type="secondary" style={{ fontSize: 11 }}>{t('trafficDetail.sse.rawJson')}</Text>,
          children: <JsonBlock data={rawBody} />,
        }]}
        style={{ backgroundColor: 'transparent', borderColor: token.colorBorderSecondary }}
      />
    </div>
  );
});

interface SseResponseViewProps {
  body: string;
  mode: 'openai' | 'trae' | 'doubao';
}

export const SseResponseView = memo(({ body, mode }: SseResponseViewProps) => {
  const parsed = useMemo(() => parseAssemblyBody(body, mode), [body, mode]);

  if (!parsed) {
    return (
      <div style={{ padding: '8px 4px' }}>
        <JsonBlock data={body} />
      </div>
    );
  }

  if (parsed.type === 'openai') {
    return <OpenAiResponsePanel data={parsed} rawBody={body} />;
  }

  if (parsed.type === 'doubao') {
    return <DouBaoResponsePanel data={parsed} rawBody={body} />;
  }

  return <TraeResponsePanel data={parsed} rawBody={body} />;
});
