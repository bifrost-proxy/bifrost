import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Space,
  Tag,
  Typography,
  theme,
} from "antd";
import { ArrowLeftOutlined } from "@ant-design/icons";
import type {
  AsrTaskFileRecord,
  AsrTimelineSegment,
  AsrTranscriptTimeline,
} from "../../../api/asr";
import { buildAsrTaskFileSourceUrl } from "../../../api/asr";
import { formatDuration, formatTime } from "../asrUtils";

const { Text, Paragraph } = Typography;
const AUTO_FOLLOW_RESUME_DELAY_MS = 5000;

interface TaskFileTranscriptPageProps {
  token: ReturnType<typeof theme.useToken>["token"];
  file: AsrTaskFileRecord | null;
  timeline: AsrTranscriptTimeline | null;
  loading: boolean;
  onBack: () => void;
}

export default function TaskFileTranscriptPage({
  token,
  file,
  timeline,
  loading,
  onBack,
}: TaskFileTranscriptPageProps) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const segmentsContainerRef = useRef<HTMLDivElement | null>(null);
  const segmentRefs = useRef<Record<number, HTMLDivElement | null>>({});
  const activeSegmentIndexRef = useRef(-1);
  const autoScrollingRef = useRef(false);
  const autoScrollFlagTimerRef = useRef<number | null>(null);
  const autoFollowResumeTimerRef = useRef<number | null>(null);
  const autoFollowPausedUntilRef = useRef(0);
  const [currentMs, setCurrentMs] = useState(0);
  const audioUrl = file ? buildAsrTaskFileSourceUrl(file.task_id, file.key) : "";
  const fullTranscript = useMemo(
    () => timeline?.segments.map((segment) => segment.text).join("\n") || "",
    [timeline],
  );
  const activeSegmentIndex = useMemo(
    () => findActiveSegmentIndex(timeline?.segments || [], currentMs),
    [currentMs, timeline],
  );
  const speakerMetadataById = useMemo(
    () => new Map((timeline?.speakers || []).map((speaker) => [speaker.id, speaker])),
    [timeline],
  );

  const syncCurrentTime = useCallback((audio: HTMLAudioElement) => {
    setCurrentMs(Math.round(audio.currentTime * 1000));
  }, []);

  const scrollToActiveSegment = useCallback(() => {
    const index = activeSegmentIndexRef.current;
    if (index < 0) {
      return;
    }
    const container = segmentsContainerRef.current;
    const segment = segmentRefs.current[index];
    if (!container || !segment) {
      return;
    }
    const containerRect = container.getBoundingClientRect();
    const segmentRect = segment.getBoundingClientRect();
    const nextTop =
      container.scrollTop +
      (segmentRect.top - containerRect.top) -
      container.clientHeight / 2 +
      segment.clientHeight / 2;
    autoScrollingRef.current = true;
    if (autoScrollFlagTimerRef.current !== null) {
      window.clearTimeout(autoScrollFlagTimerRef.current);
    }
    container.scrollTo({
      top: Math.max(0, nextTop),
      behavior: "auto",
    });
    autoScrollFlagTimerRef.current = window.setTimeout(() => {
      autoScrollingRef.current = false;
      autoScrollFlagTimerRef.current = null;
    }, 120);
  }, []);

  useEffect(() => {
    activeSegmentIndexRef.current = activeSegmentIndex;
  }, [activeSegmentIndex]);

  useEffect(() => {
    if (Date.now() < autoFollowPausedUntilRef.current) {
      return;
    }
    scrollToActiveSegment();
  }, [activeSegmentIndex, scrollToActiveSegment]);

  useEffect(
    () => () => {
      if (autoScrollFlagTimerRef.current !== null) {
        window.clearTimeout(autoScrollFlagTimerRef.current);
      }
      if (autoFollowResumeTimerRef.current !== null) {
        window.clearTimeout(autoFollowResumeTimerRef.current);
      }
    },
    [],
  );

  const pauseAutoFollow = useCallback(() => {
    autoFollowPausedUntilRef.current = Date.now() + AUTO_FOLLOW_RESUME_DELAY_MS;
    if (autoFollowResumeTimerRef.current !== null) {
      window.clearTimeout(autoFollowResumeTimerRef.current);
    }
    autoFollowResumeTimerRef.current = window.setTimeout(() => {
      autoFollowPausedUntilRef.current = 0;
      autoFollowResumeTimerRef.current = null;
      scrollToActiveSegment();
    }, AUTO_FOLLOW_RESUME_DELAY_MS);
  }, [scrollToActiveSegment]);

  const pauseAutoFollowForManualScroll = useCallback(() => {
    if (autoScrollingRef.current) {
      return;
    }
    pauseAutoFollow();
  }, [pauseAutoFollow]);

  const resumeAutoFollow = useCallback(
    (nextCurrentMs?: number) => {
      autoFollowPausedUntilRef.current = 0;
      if (autoFollowResumeTimerRef.current !== null) {
        window.clearTimeout(autoFollowResumeTimerRef.current);
        autoFollowResumeTimerRef.current = null;
      }
      if (nextCurrentMs !== undefined) {
        activeSegmentIndexRef.current = findActiveSegmentIndex(
          timeline?.segments || [],
          nextCurrentMs,
        );
      }
      window.requestAnimationFrame(scrollToActiveSegment);
    },
    [scrollToActiveSegment, timeline],
  );

  const syncCurrentTimeAndResumeFollow = useCallback(
    (audio: HTMLAudioElement) => {
      const nextCurrentMs = Math.round(audio.currentTime * 1000);
      setCurrentMs(nextCurrentMs);
      resumeAutoFollow(nextCurrentMs);
    },
    [resumeAutoFollow],
  );

  const jumpToSegment = (segment: AsrTimelineSegment) => {
    const audio = audioRef.current;
    if (!audio) {
      return;
    }
    try {
      audio.currentTime = segment.audio_start_ms / 1000;
    } catch {
      return;
    }
    setCurrentMs(segment.audio_start_ms);
    resumeAutoFollow(segment.audio_start_ms);
    void audio.play().catch(() => undefined);
  };

  if (!file) {
    return (
      <Card
        title={
          <Space>
            <Button icon={<ArrowLeftOutlined />} onClick={onBack} aria-label="Back to task files" />
            <span>Transcript File</span>
          </Space>
        }
      >
        <Text type="secondary">Select a processed file to inspect its transcript timeline.</Text>
      </Card>
    );
  }

  return (
    <Card
      data-testid="asr-task-file-detail-page"
      title={
        <Space>
          <Button icon={<ArrowLeftOutlined />} onClick={onBack} aria-label="Back to task files" />
          <span>Transcript File</span>
        </Space>
      }
    >
      <Space direction="vertical" size={16} style={{ width: "100%" }}>
        <section
          aria-label="Original audio"
          style={{
            border: `1px solid ${token.colorBorderSecondary}`,
            borderRadius: 6,
            padding: 16,
            background: token.colorFillQuaternary,
          }}
        >
          <Space
            align="center"
            style={{ width: "100%", justifyContent: "space-between", marginBottom: 12 }}
          >
            <Typography.Title level={5} style={{ margin: 0 }}>
              Original Audio
            </Typography.Title>
            <Tag>{formatDuration(file.media_duration_ms)}</Tag>
          </Space>
          <audio
            ref={audioRef}
            src={audioUrl}
            controls
            preload="metadata"
            aria-label="Original audio player"
            style={{ width: "100%" }}
            onTimeUpdate={(event) => {
              syncCurrentTime(event.currentTarget);
            }}
            onPlay={(event) => {
              syncCurrentTimeAndResumeFollow(event.currentTarget);
            }}
            onSeeking={(event) => {
              syncCurrentTimeAndResumeFollow(event.currentTarget);
            }}
            onSeeked={(event) => {
              syncCurrentTimeAndResumeFollow(event.currentTarget);
            }}
          />
          <Text type="secondary" style={{ display: "block", marginTop: 8, fontSize: 12 }}>
            {file.source_path}
          </Text>
        </section>

        {loading ? (
          <Text type="secondary">Loading transcript timeline...</Text>
        ) : timeline ? (
          <section
            aria-label="Transcript timeline"
            style={{
              border: `1px solid ${token.colorBorderSecondary}`,
              borderRadius: 6,
              padding: 16,
              background: token.colorFillQuaternary,
            }}
          >
            <Space
              align="center"
              style={{ width: "100%", justifyContent: "space-between", marginBottom: 12 }}
            >
              <Typography.Title level={5} style={{ margin: 0 }}>
                File Timeline
              </Typography.Title>
              <Tag color="blue">{timeline.segments.length} segments</Tag>
            </Space>
            <Descriptions size="small" bordered column={2} style={{ marginBottom: 12 }}>
              <Descriptions.Item label="File" span={2}>
                {timeline.source_path}
              </Descriptions.Item>
              <Descriptions.Item label="Recorded">
                {formatTime(timeline.source_created_at_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="Source">
                {timeline.source_created_at_source || "-"}
              </Descriptions.Item>
              <Descriptions.Item label="Duration">
                {formatDuration(timeline.media_duration_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="Segments">
                {timeline.segments.length}
              </Descriptions.Item>
              <Descriptions.Item label="Speakers">
                {timeline.speakers?.length ? (
                  <Space wrap>
                    {timeline.speakers.map((speaker) => (
                      <Tag key={speaker.id} color="purple">
                        {formatSpeakerLabel(speaker.display_name, speaker.confidence)}
                        {!speaker.mapped_profile_id
                          ? formatSpeakerCandidate(speaker.candidate_display_name, speaker.candidate_confidence)
                          : ""}
                      </Tag>
                    ))}
                  </Space>
                ) : (
                  "-"
                )}
              </Descriptions.Item>
            </Descriptions>
            <div>
              <Text strong>Segments</Text>
              <div
                ref={segmentsContainerRef}
                data-testid="asr-transcript-segments"
                onScroll={pauseAutoFollowForManualScroll}
                onWheel={pauseAutoFollow}
                onTouchMove={pauseAutoFollow}
                style={{
                  marginTop: 12,
                  maxHeight: 460,
                  overflow: "auto",
                  paddingRight: 8,
                }}
              >
                {timeline.segments.length > 0 ? (
                  <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                    {timeline.segments.map((segment, index) => (
                      <div
                        key={index}
                        ref={(element) => {
                          segmentRefs.current[index] = element;
                        }}
                        data-testid={`asr-transcript-segment-${index}`}
                        aria-current={index === activeSegmentIndex ? "true" : undefined}
                        style={{
                          display: "flex",
                          alignItems: "baseline",
                          gap: 8,
                          padding: "3px 6px",
                          borderRadius: 4,
                          background: index === activeSegmentIndex ? token.colorPrimaryBg : "transparent",
                          transition: "background 0.2s",
                        }}
                      >
                        <Button
                          type="link"
                          size="small"
                          style={{
                            flexShrink: 0,
                            padding: 0,
                            height: "auto",
                            fontSize: 12,
                            fontFamily: "monospace",
                            color: index === activeSegmentIndex ? token.colorPrimary : token.colorTextSecondary,
                          }}
                          onClick={() => jumpToSegment(segment)}
                        >
                          {formatDuration(segment.audio_start_ms)}
                        </Button>
                        {segment.speaker ? (
                          <Tag color="purple" style={{ flexShrink: 0, marginInlineEnd: 0 }}>
                            {formatSpeakerLabel(
                              segment.speaker_display_name ?? segment.speaker,
                              speakerMetadataById.get(segment.speaker)?.confidence,
                            )}
                            {!speakerMetadataById.get(segment.speaker)?.mapped_profile_id
                              ? formatSpeakerCandidate(
                                  speakerMetadataById.get(segment.speaker)?.candidate_display_name,
                                  speakerMetadataById.get(segment.speaker)?.candidate_confidence,
                                )
                              : ""}
                          </Tag>
                        ) : null}
                        <Paragraph
                          style={{
                            flex: 1,
                            marginBottom: 0,
                            whiteSpace: "pre-wrap",
                            minWidth: 0,
                            lineHeight: 1.6,
                          }}
                        >
                          {segment.text}
                        </Paragraph>
                      </div>
                    ))}
                  </div>
                ) : (
                  <Text type="secondary">No transcript segments were produced.</Text>
                )}
              </div>
            </div>
            <div style={{ marginTop: 16 }}>
              <Text strong>Full Transcript</Text>
              <Paragraph
                style={{
                  marginTop: 12,
                  marginBottom: 0,
                  padding: 12,
                  whiteSpace: "pre-wrap",
                  background: token.colorBgContainer,
                  border: `1px solid ${token.colorBorderSecondary}`,
                  borderRadius: 6,
                }}
              >
                {fullTranscript || "No transcript text."}
              </Paragraph>
            </div>
          </section>
        ) : (
          <Alert
            type="warning"
            showIcon
            message="Transcript timeline is unavailable"
            description="This file has not produced a timeline yet, or the saved timeline could not be loaded."
          />
        )}
      </Space>
    </Card>
  );
}

function findActiveSegmentIndex(segments: AsrTimelineSegment[], currentMs: number): number {
  if (segments.length === 0) {
    return -1;
  }
  const containedIndex = segments.findIndex(
    (segment, index) =>
      currentMs >= segment.audio_start_ms &&
      (currentMs < segment.audio_end_ms || index === segments.length - 1),
  );
  if (containedIndex >= 0) {
    return containedIndex;
  }
  const nextIndex = segments.findIndex((segment) => currentMs < segment.audio_start_ms);
  return nextIndex >= 0 ? nextIndex : segments.length - 1;
}

function formatSpeakerLabel(displayName: string, confidence?: number): string {
  if (confidence === undefined || confidence === null) {
    return displayName;
  }
  return `${displayName} ${Math.round(Math.max(0, Math.min(1, confidence)) * 100)}%`;
}

function formatSpeakerCandidate(displayName?: string, confidence?: number): string {
  if (!displayName || confidence === undefined || confidence === null) {
    return "";
  }
  return ` candidate ${displayName} ${Math.round(Math.max(0, Math.min(1, confidence)) * 100)}%`;
}
