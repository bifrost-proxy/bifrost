import { expect, type Page, test } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

async function installAsrMicrophoneMocks(page: Page) {
  await page.route("**/_bifrost/api/asr/status**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        status: "ready",
        ready: true,
        installed: true,
        managed: true,
        server_url: "http://127.0.0.1:54321",
        install_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs",
        model_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs/Qwen3-ASR-1.7B",
        model: "Qwen3-ASR-1.7B",
        language: "chinese",
        message: "Qwen3-ASR files are installed and the service is healthy.",
      }),
    });
  });

  await page.addInitScript({
    content: `
      (() => {
        window.localStorage.removeItem("bifrost.asr.connection.v3");
        window.__asrStoppedTracks = 0;
        window.__asrClosedAudioContexts = 0;
        window.__asrVoiceSocketUrls = [];
        window.__asrVoiceStartMessages = [];
        window.__asrVoiceBinaryBytes = [];

        class FakeAnalyser {
          constructor() {
            this.fftSize = 1024;
            this.smoothingTimeConstant = 0.72;
            this.frequencyBinCount = 64;
            this._tick = 0;
          }

          getByteFrequencyData(samples) {
            this._tick += 1;
            for (let index = 0; index < samples.length; index += 1) {
              samples[index] = 48 + ((index * 9 + this._tick * 17) % 172);
            }
          }
        }

        class FakeAudioContext {
          constructor(options) {
            this.sampleRate = options?.sampleRate || 16000;
            this.destination = { kind: "destination" };
            this._processors = new Set();
          }

          createAnalyser() {
            return new FakeAnalyser();
          }

          createMediaStreamSource() {
            return {
              connect: (target) => {
                if (typeof target?.onaudioprocess !== "undefined") {
                  const processor = target;
                  processor._timer = window.setInterval(() => {
                    const samples = new Float32Array(160);
                    for (let index = 0; index < samples.length; index += 1) {
                      samples[index] = Math.sin((index / samples.length) * Math.PI * 2) * 0.2;
                    }
                    processor.onaudioprocess?.({
                      inputBuffer: {
                        getChannelData: () => samples,
                      },
                      outputBuffer: {
                        getChannelData: () => new Float32Array(samples.length),
                      },
                    });
                  }, 40);
                  this._processors.add(processor);
                }
              },
              disconnect() {},
            };
          }

          createScriptProcessor() {
            const processor = {
              onaudioprocess: null,
              _timer: null,
              connect() {},
              disconnect() {
                if (this._timer) {
                  window.clearInterval(this._timer);
                  this._timer = null;
                }
              },
            };
            return processor;
          }

          close() {
            for (const processor of this._processors) {
              processor.disconnect();
            }
            this._processors.clear();
            window.__asrClosedAudioContexts += 1;
            return Promise.resolve();
          }
        }

        Object.defineProperty(window, "AudioContext", {
          value: FakeAudioContext,
          configurable: true,
        });
        Object.defineProperty(window, "webkitAudioContext", {
          value: FakeAudioContext,
          configurable: true,
        });
        Object.defineProperty(navigator, "mediaDevices", {
          value: {
            getUserMedia: async () => ({
              getTracks: () => [
                {
                  stop() {
                    window.__asrStoppedTracks += 1;
                  },
                },
              ],
            }),
          },
          configurable: true,
        });

        class FakeWebSocket {
          static CONNECTING = 0;
          static OPEN = 1;
          static CLOSING = 2;
          static CLOSED = 3;

          constructor(url) {
            this.url = String(url);
            this.readyState = FakeWebSocket.CONNECTING;
            window.__asrVoiceSocketUrls.push(this.url);
            window.setTimeout(() => {
              this.readyState = FakeWebSocket.OPEN;
              this.onopen?.();
              this._emit({
                type: "connected",
                phase: "connected",
                status: "ok",
                progress: 1,
                message: "mock microphone stream connected",
              });
            }, 20);
          }

          send(payload) {
            if (typeof payload === "string") {
              const message = JSON.parse(payload);
              if (message.type === "start") {
                window.__asrVoiceStartMessages.push(message);
                this._emit({
                  type: "source_ready",
                  source: "web_mic",
                  message: "mock voice source started",
                  detail: "sample_rate=16000; channels=1; format=pcm_s16le",
                });
                window.setTimeout(() => {
                  this._emit({
                    type: "asr_partial",
                    window_index: 1,
                    captured_at_ms: 400,
                    emitted_at_ms: 450,
                    text: "测试",
                    delta: "测试",
                    committed: "",
                  });
                }, 120);
              } else if (message.type === "finish") {
                this._emit({
                  type: "asr_final_utterance",
                  window_index: 1,
                  emitted_at_ms: 1000,
                  text: "测试",
                  delta: "测试",
                  committed: "测试",
                });
                this._emit({ type: "done", ok: true });
                this.close();
              } else if (message.type === "cancel") {
                this.close();
              }
              return;
            }

            window.__asrVoiceBinaryBytes.push(payload.byteLength || payload.size || 0);
            this._emit({
              type: "asr_partial",
              window_index: 2,
              captured_at_ms: 800,
              emitted_at_ms: 850,
              text: "测试",
              delta: "测试",
              committed: "",
            });
          }

          close() {
            if (this.readyState === FakeWebSocket.CLOSED) {
              return;
            }
            this.readyState = FakeWebSocket.CLOSED;
            this.onclose?.();
          }

          _emit(message) {
            if (this.readyState !== FakeWebSocket.OPEN) {
              return;
            }
            this.onmessage?.({ data: JSON.stringify(message) });
          }
        }

        Object.defineProperty(window, "WebSocket", {
          value: FakeWebSocket,
          configurable: true,
        });
      })();
    `,
  });
}

async function waitForLiveMeter(page: Page) {
  await page.waitForFunction(() => {
    const text = document
      .querySelector('[aria-label="Microphone input level"]')
      ?.textContent;
    return !!text && /([1-9][0-9]?|100)%/.test(text);
  });
}

async function installAsrFileTranscriptionMock(page: Page) {
  await page.route("**/_bifrost/api/asr/transcribe-stream**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body: [
        'event: progress\ndata: {"phase":"upload","status":"ok","progress":35,"message":"Uploading audio"}',
        'event: progress\ndata: {"phase":"transcribe","status":"ok","progress":70,"message":"Transcribing audio"}',
        'event: text\ndata: {"text":"测试文件"}',
        'event: done\ndata: {"ok":true}',
        "",
      ].join("\n\n"),
    });
  });
}

test("ASR microphone input shows live level meter and resets on stop/cancel", async ({
  page,
}) => {
  await installAsrMicrophoneMocks(page);
  await installAsrFileTranscriptionMock(page);
  await openPage(page, "ai?aiSection=tools-asr");

  const workbench = page.getByTestId("asr-workbench-card");
  await expect(workbench).toBeVisible();
  await expect(workbench.getByText("Audio Input", { exact: true })).toBeVisible();
  await expect(workbench.getByText("Transcript", { exact: true })).toBeVisible();
  const workbenchLayout = await workbench.evaluate((card) => {
    const input = card
      .querySelector('[aria-label="Audio Input"]')
      ?.getBoundingClientRect();
    const transcript = card
      .querySelector('[aria-label="Transcript"]')
      ?.getBoundingClientRect();
    return {
      hasInput: Boolean(input),
      hasTranscript: Boolean(transcript),
      inputBottom: input?.bottom ?? 0,
      transcriptTop: transcript?.top ?? 0,
    };
  });
  expect(workbenchLayout.hasInput).toBe(true);
  expect(workbenchLayout.hasTranscript).toBe(true);
  expect(workbenchLayout.transcriptTop).toBeGreaterThanOrEqual(
    workbenchLayout.inputBottom,
  );
  await expect(page.getByTestId("asr-file-progress")).toHaveCount(0);

  const meter = page.locator('[aria-label="Microphone input level"]');
  await expect(meter).toBeVisible();
  await expect(meter).toContainText("Mic level");
  await expect(meter).toContainText("0%");

  await page.getByRole("button", { name: "Start Mic" }).click();
  await expect(page.getByRole("button", { name: "Stop Mic" })).toBeVisible();
  await expect(page.getByTestId("asr-file-progress")).toHaveCount(0);
  await expect(meter).toContainText("Live microphone level");
  await waitForLiveMeter(page);
  await expect(page.getByText("partial[1]: captured 400ms")).toBeVisible();

  await page.getByRole("button", { name: "Stop Mic" }).click();
  await expect(meter).toContainText("Mic level");
  await expect(meter).toContainText("0%");

  await page.getByRole("button", { name: "Start Mic" }).click();
  await waitForLiveMeter(page);
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(meter).toContainText("Mic level");
  await expect(meter).toContainText("0%");

  const cleanupCounts = await page.evaluate(() => ({
    stoppedTracks: (window as Window & { __asrStoppedTracks: number })
      .__asrStoppedTracks,
    closedAudioContexts: (window as Window & { __asrClosedAudioContexts: number })
      .__asrClosedAudioContexts,
    voiceSocketUrls: (window as Window & { __asrVoiceSocketUrls: string[] })
      .__asrVoiceSocketUrls,
    voiceStartMessages: (
      window as Window & { __asrVoiceStartMessages: Array<Record<string, unknown>> }
    ).__asrVoiceStartMessages,
    voiceBinaryBytes: (window as Window & { __asrVoiceBinaryBytes: number[] })
      .__asrVoiceBinaryBytes,
  }));
  expect(cleanupCounts.stoppedTracks).toBeGreaterThanOrEqual(2);
  expect(cleanupCounts.closedAudioContexts).toBeGreaterThanOrEqual(2);
  const voiceSocketUrl = cleanupCounts.voiceSocketUrls.find((url) =>
    url.includes("/api/voice/listen-ws"),
  );
  expect(voiceSocketUrl).toBeTruthy();
  expect(voiceSocketUrl).toContain("provider=qwen3_stateful_streaming");
  expect(voiceSocketUrl).toContain("model=Qwen3-ASR-0.6B");
  expect(cleanupCounts.voiceStartMessages[0]).toMatchObject({
    type: "start",
    source: "web_mic",
    sample_rate: 16000,
    channels: 1,
    format: "pcm_s16le",
  });
  expect(cleanupCounts.voiceBinaryBytes.some((bytes) => bytes > 0)).toBe(true);

  await page.locator('input[type="file"]').setInputFiles({
    name: "sample.wav",
    mimeType: "audio/wav",
    buffer: Buffer.from("RIFF....WAVEfmt "),
  });
  const fileProgress = page.getByTestId("asr-file-progress");
  await expect(fileProgress).toBeVisible();
  await expect(fileProgress).toContainText("sample.wav");
  await expect(page.getByText("测试文件")).toBeVisible();
});

test("ASR microphone level meter remains readable in dark theme", async ({ page }) => {
  await installAsrMicrophoneMocks(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "bifrost-theme",
      JSON.stringify({ state: { mode: "dark" }, version: 0 }),
    );
  });
  await openPage(page, "ai?aiSection=tools-asr");

  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  const meter = page.locator('[aria-label="Microphone input level"]');
  await expect(meter).toBeVisible();
  await expect(meter).toContainText("Mic level");
  await expect(meter).toContainText("0%");

  await page.getByRole("button", { name: "Start Mic" }).click();
  await expect(meter).toContainText("Live microphone level");
  await waitForLiveMeter(page);
});

test("ASR directory tasks can be created and refreshed in the tools panel", async ({
  page,
}) => {
  await installAsrMicrophoneMocks(page);
  let created = false;
  let cleanedSourceAudio = false;
  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    if (route.request().method() === "POST") {
      created = true;
      const body = JSON.parse(route.request().postData() ?? "{}");
      expect(body.interval_seconds).toBeUndefined();
      expect(body.schedule).toEqual({
        kind: "daily",
        hour: 2,
        minute: 0,
      });
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          id: "task-1",
          name: "Recordings",
          audio_dir: "/tmp/asr-audio",
          recursive: true,
          enabled: true,
          schedule: { kind: "daily", hour: 2, minute: 0 },
          language: "chinese",
          model: "Qwen3-ASR-1.7B",
          created_at_ms: Date.now(),
          updated_at_ms: Date.now(),
          next_run_at_ms: Date.now() + 60000,
          summary: {
            discovered: 1,
            processed: 0,
            pending: 1,
            failed: 0,
            partial_success: 0,
            failed_chunk_count: 0,
            deleted_after_processing: 0,
            audio_source_bytes: 0,
            audio_source_file_count: 1,
            cleanable_source_bytes: 0,
            cleanable_source_file_count: 0,
            running: false,
          },
        }),
      });
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        tasks: created
          ? [
              {
                id: "task-1",
                name: "Recordings",
                audio_dir: "/tmp/asr-audio",
                recursive: true,
                enabled: true,
                schedule: { kind: "daily", hour: 2, minute: 0 },
                language: "chinese",
                model: "Qwen3-ASR-1.7B",
                created_at_ms: Date.now(),
                updated_at_ms: Date.now(),
                next_run_at_ms: Date.now() + 60000,
                summary: {
                  discovered: 1,
                  processed: 1,
                  pending: 0,
                  failed: 0,
                  partial_success: 0,
                  failed_chunk_count: 0,
                  deleted_after_processing: 1,
                  audio_source_bytes: 0,
                  audio_source_file_count: 0,
                  cleanable_source_bytes: 0,
                  cleanable_source_file_count: 0,
                  running: false,
                },
              },
            ]
          : [],
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/run", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        processed_now: 1,
        failed_now: 0,
        message: "ok",
        task: {
          id: "task-1",
          name: "Recordings",
          audio_dir: "/tmp/asr-audio",
          recursive: true,
          enabled: true,
          schedule: { kind: "daily", hour: 2, minute: 0 },
          language: "chinese",
          model: "Qwen3-ASR-1.7B",
          created_at_ms: Date.now(),
          updated_at_ms: Date.now(),
          next_run_at_ms: Date.now() + 60000,
          summary: {
            discovered: 1,
            processed: 1,
            pending: 0,
            failed: 0,
            partial_success: 0,
            failed_chunk_count: 0,
            deleted_after_processing: 1,
            audio_source_bytes: 0,
            audio_source_file_count: 0,
            cleanable_source_bytes: 0,
            cleanable_source_file_count: 0,
            running: false,
          },
        },
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/cleanup-source-audio", async (route) => {
    cleanedSourceAudio = true;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ok: true,
        deleted_files: 1,
        deleted_bytes: 1234,
        skipped_files: 19,
        skipped_bytes: 23456,
        failed_files: [],
        message: "Deleted 1 ASR source audio file(s).",
        summary: {
          discovered: 19,
          processed: 1,
          pending: 0,
          failed: 0,
          partial_success: 0,
          failed_chunk_count: 0,
          deleted_after_processing: 2,
          audio_source_bytes: 23456,
          audio_source_file_count: 19,
          cleanable_source_bytes: 0,
          cleanable_source_file_count: 0,
          running: false,
        },
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1", async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: "task-1",
        name: "Recordings",
        audio_dir: "/tmp/asr-audio",
        recursive: true,
        enabled: true,
        schedule: { kind: "daily", hour: 2, minute: 0 },
        language: "chinese",
        model: "Qwen3-ASR-1.7B",
        created_at_ms: Date.now(),
        updated_at_ms: Date.now(),
        last_run_at_ms: Date.now() - 30000,
        next_run_at_ms: Date.now() + 86400000,
        summary: {
          discovered: 1,
          processed: 1,
          pending: 0,
          failed: 0,
          partial_success: 0,
          failed_chunk_count: 0,
          deleted_after_processing: 1,
          audio_source_bytes: cleanedSourceAudio ? 23456 : 24690,
          audio_source_file_count: cleanedSourceAudio ? 19 : 20,
          cleanable_source_bytes: cleanedSourceAudio ? 0 : 1234,
          cleanable_source_file_count: cleanedSourceAudio ? 0 : 1,
          running: false,
        },
        daily_documents: [
          {
            date: "2026-05-14",
            path: "/tmp/bifrost/asr/data/text/task-1/.daily/2026-05-14.md",
            size: 256,
            modified_ms: Date.now() - 5000,
            text_chars: 42,
          },
        ],
        files: Array.from({ length: 20 }, (_item, index) => ({
          key: `file-${index + 1}`,
          task_id: "task-1",
          source_path: `/tmp/asr-audio/sample-${index + 1}.wav`,
          source_size: 1234 + index,
          source_modified_ms: Date.now() - 60000,
          source_created_at_ms: new Date("2026-05-14T11:44:33+08:00").getTime() + index * 1000,
          source_created_at_source: "ffprobe.date_creation_time",
          media_duration_ms: 12000,
          status: "success",
          output_text_path: `/tmp/bifrost/asr/data/text/task-1/file-${index + 1}.txt`,
          output_metadata_path: `/tmp/bifrost/asr/data/text/task-1/file-${index + 1}.json`,
          output_timeline_path: `/tmp/bifrost/asr/data/text/task-1/file-${index + 1}.timeline.json`,
          text_chars: 4,
          finished_at_ms: Date.now() - 30000,
        })),
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/daily/2026-05-14", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: "task-1",
        task_name: "Recordings",
        date: "2026-05-14",
        path: "/tmp/bifrost/asr/data/text/task-1/.daily/2026-05-14.md",
        size: 256,
        modified_ms: Date.now() - 5000,
        text_chars: 42,
        content:
          "# Recordings — 2026-05-14\n\n## sample-1\n\n**[2026-05-14 11:44:33.000 → 2026-05-14 11:44:45.000]**  \n完整按天整理内容\n",
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/files/file-1/timeline", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: "task-1",
        task_name: "Recordings",
        source_path: "/tmp/asr-audio/sample.wav",
        source_size: 1234,
        source_modified_ms: Date.now() - 60000,
        source_created_at_ms: new Date("2026-05-14T11:44:33+08:00").getTime(),
        source_created_at_source: "ffprobe.date_creation_time",
        media_duration_ms: 12000,
        model: "Qwen3-ASR-1.7B",
        language: "chinese",
        processed_at_ms: Date.now(),
        segments: Array.from({ length: 12 }, (_item, index) => ({
          index,
          audio_start_ms: index * 1000,
          audio_end_ms: (index + 1) * 1000,
          absolute_start_ms:
            new Date("2026-05-14T11:44:33+08:00").getTime() + index * 1000,
          absolute_end_ms:
            new Date("2026-05-14T11:44:33+08:00").getTime() + (index + 1) * 1000,
          text:
            index === 0 ? "时间线文本" : index === 1 ? "第二段文本" : `第${index + 1}段文本`,
        })),
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/files/file-1/source", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "audio/wav",
      body: Buffer.from("RIFF....WAVEfmt "),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/chat/config", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ runners: {} }),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/providers", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([]),
    });
  });
  await page.route("**/_bifrost/api/im-gateway/targets", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([]),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/daily-agent", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: "task-1",
        config: {
          enabled: true,
          runner: "bifrost_agent",
          timeout_ms: 7200000,
          trigger_policy: "after_asr_run",
          instructions_source: "default",
          im_delivery: {
            enabled: false,
            mode: "summary",
            send_policy: "on_success_with_report",
          },
        },
        workspace: {
          daily_dir: "/tmp/bifrost/asr/data/text/task-1/.daily",
          report_dir: "/tmp/bifrost/asr/data/text/task-1/.daily/report",
          agents_path: "/tmp/bifrost/asr/data/text/task-1/.daily/AGENTS.md",
          agents_exists: true,
          git_available: true,
          git_initialized: true,
          report_count: 1,
        },
        last_run: {
          run_id: "run-20260514",
          status: "success",
          last_run_at_ms: Date.now() - 1000,
        },
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/daily-agent/agents", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: "task-1",
        content: "# Daily Agent instructions\n",
        source: "file",
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/task-1/daily-agent/runs", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        task_id: "task-1",
        processed_documents: [
          {
            date: "2026-05-13",
            source_sha256: "old1234567890",
            source_len_bytes: 128,
            processed_at_ms: Date.now() - 3000,
            runner: "bifrost_agent",
            report_path:
              "/tmp/bifrost/asr/data/text/task-1/.daily/report/2026-05-13-report.md",
            last_run_id: "run-20260513",
          },
          {
            date: "2026-05-14",
            source_sha256: "abcdef1234567890",
            source_len_bytes: 256,
            processed_at_ms: Date.now() - 1000,
            runner: "bifrost_agent",
            report_path:
              "/tmp/bifrost/asr/data/text/task-1/.daily/report/2026-05-14-report.md",
            last_run_id: "run-20260514",
          },
          {
            date: "2026-05-15",
            source_sha256: "new1234567890",
            source_len_bytes: 512,
            processed_at_ms: Date.now() - 500,
            runner: "bifrost_agent",
            report_path:
              "/tmp/bifrost/asr/data/text/task-1/.daily/report/2026-05-15-report.md",
            last_run_id: "run-20260515",
          },
        ],
      }),
    });
  });
  await page.route(
    "**/_bifrost/api/asr/tasks/task-1/daily-agent/reports/2026-05-14",
    async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          task_id: "task-1",
          task_name: "Recordings",
          date: "2026-05-14",
          path: "/tmp/bifrost/asr/data/text/task-1/.daily/report/2026-05-14-report.md",
          size: 128,
          modified_ms: Date.now() - 500,
          content: "# Daily Agent Report\n\n运行记录内容",
          processed_at_ms: Date.now() - 1000,
          runner: "bifrost_agent",
          last_run_id: "run-20260514",
        }),
      });
    },
  );

  await openPage(page, "ai?aiSection=tools-asr");
  await expect(page.getByText("Directory Tasks")).toBeVisible();
  const speechConverterTop = await page
    .getByText("Speech Converter", { exact: true })
    .boundingBox();
  const directoryTasksTop = await page
    .getByText("Directory Tasks", { exact: true })
    .boundingBox();
  const speechToTextTop = await page.getByText("Speech to Text", { exact: true }).boundingBox();
  expect(speechConverterTop?.y).toBeLessThan(directoryTasksTop?.y ?? Infinity);
  expect(directoryTasksTop?.y).toBeLessThan(speechToTextTop?.y ?? Infinity);
  await expect(page.getByPlaceholder("Meeting audio watcher")).toHaveCount(0);
  await page.getByRole("button", { name: "New" }).click();
  const createDialog = page.getByRole("dialog", { name: "New Directory Task" });
  await expect(createDialog).toBeVisible();
  await createDialog.getByTestId("asr-runtime-strategy-select").click();
  const runtimeDropdown = page.locator(".ant-select-dropdown:visible");
  await expect(runtimeDropdown.getByText("Reuse / file")).toBeVisible();
  await expect(runtimeDropdown.getByText("Default for most offline tasks.")).toBeVisible();
  await expect(runtimeDropdown.getByText("Fork / chunk")).toBeVisible();
  await expect(runtimeDropdown.getByText("fresh ASR process for every chunk")).toBeVisible();
  await expect(runtimeDropdown.getByText("Reuse server")).toBeVisible();
  await expect(runtimeDropdown.getByText("memory or server-state issues")).toBeVisible();
  await expect(runtimeDropdown.getByText("Auto fallback")).toBeVisible();
  await expect(runtimeDropdown.getByText("automatically falls back")).toBeVisible();
  await expect(runtimeDropdown.getByText("Compare")).toBeVisible();
  await expect(runtimeDropdown.getByText("Diagnostic mode.")).toBeVisible();
  await page.keyboard.press("Escape");
  await createDialog.getByPlaceholder("Meeting audio watcher").fill("Recordings");
  await createDialog.getByPlaceholder("~/Recordings").fill("/tmp/asr-audio");
  await createDialog.getByRole("button", { name: "Create" }).click();
  await expect(createDialog).toHaveCount(0);
  await expect(page.getByText("Recordings")).toBeVisible();
  await expect(page.getByText("/tmp/asr-audio")).toBeVisible();
  await expect(page.getByText("Daily at 02:00")).toBeVisible();
  await expect(page.getByText(/processed 1, pending 0/)).toBeVisible();
  await expect(page.getByText(/deleted after processing 1/)).toBeVisible();
  await page.getByRole("button", { name: "View details" }).click();
  await expect(page).toHaveURL(/asrTask=task-1/);
  await expect(page.getByRole("dialog", { name: "Directory Task: Recordings" })).toHaveCount(0);
  const taskPage = page.getByTestId("asr-task-detail-page");
  await expect(taskPage).toBeVisible();
  await expect(taskPage.getByText("Directory Task: Recordings")).toBeVisible();
  await expect(taskPage.getByText("Audio Files")).toBeVisible();
  await expect(taskPage.getByText(/24\.1 KB/)).toBeVisible();
  await expect(taskPage.getByText("Cleanable Originals")).toBeVisible();
  await expect(taskPage.getByText(/1\.21 KB/).first()).toBeVisible();
  await taskPage.getByRole("button", { name: "Clean originals" }).click();
  await page.getByRole("button", { name: "OK" }).click();
  await expect(page.getByText(/Deleted 1 source file/)).toBeVisible();
  await expect(taskPage.getByText(/22\.9 KB/)).toBeVisible();
  await expect(taskPage.getByText("/tmp/asr-audio/sample-1.wav")).toBeVisible();
  await expect(taskPage.getByText("/tmp/bifrost/asr/data/text/task-1/file-1.txt")).toBeVisible();
  const filesTable = taskPage.getByTestId("asr-task-files-table");
  await expect(filesTable.getByText("success").first()).toBeVisible();
  await expect(filesTable.locator(".ant-table-content")).toHaveCSS("overflow-x", "auto");
  expect(
    await filesTable.locator(".ant-table-content").evaluate((element) => element.scrollWidth),
  ).toBeGreaterThan(
    await filesTable.locator(".ant-table-content").evaluate((element) => element.clientWidth),
  );
  await expect(filesTable.locator(".ant-table-tbody tr.ant-table-row")).toHaveCount(8);
  await filesTable.locator(".ant-pagination-options-size-changer").click();
  await page
    .locator(".ant-select-dropdown:not(.ant-select-dropdown-hidden)")
    .getByText("20 / page", { exact: true })
    .click();
  await expect(filesTable.locator(".ant-table-tbody tr.ant-table-row")).toHaveCount(20);
  await taskPage.getByRole("tab", { name: /Daily Docs/ }).click();
  const dailyDocs = taskPage.getByTestId("asr-task-daily-docs-tab");
  await expect(dailyDocs.getByText("2026-05-14", { exact: true })).toBeVisible();
  await expect(
    dailyDocs.getByText("/tmp/bifrost/asr/data/text/task-1/.daily/2026-05-14.md"),
  ).toBeVisible();
  await dailyDocs.getByRole("button", { name: "Open document" }).click();
  await expect(page).toHaveURL(/asrDay=2026-05-14/);
  const dailyDocumentPage = page.getByTestId("asr-task-daily-document-page");
  await expect(dailyDocumentPage).toBeVisible();
  await expect(page.getByTestId("asr-daily-document-content")).toContainText(
    "完整按天整理内容",
  );
  await page.getByRole("button", { name: "Back to daily docs" }).click();
  await expect(page).not.toHaveURL(/asrDay=2026-05-14/);
  await taskPage.getByRole("tab", { name: "Daily Agent", exact: true }).click();
  await expect(taskPage.getByText("Configuration")).toBeVisible();
  await expect(taskPage.getByText("Agent Instructions (AGENTS.md)")).toBeVisible();
  await expect(taskPage.getByText("Processed Documents")).toHaveCount(0);
  await taskPage.getByRole("tab", { name: "Daily Agent Records", exact: true }).click();
  await expect(taskPage.getByText("Run Results")).toBeVisible();
  const runRows = taskPage
    .getByTestId("asr-daily-agent-run-results-table")
    .locator(".ant-table-tbody tr.ant-table-row");
  await expect(runRows.first().locator("td").first()).toHaveText("2026-05-15");
  const reportLink = page.getByTestId("asr-daily-agent-report-link-2026-05-14");
  await expect(reportLink).toBeVisible();
  await reportLink.click();
  await expect(page).toHaveURL(/asrTaskTab=daily-agent-records/);
  await expect(page).toHaveURL(/asrDailyReport=2026-05-14/);
  await expect(page.getByTestId("asr-daily-agent-report-content")).toContainText(
    "运行记录内容",
  );
  await page.getByRole("button", { name: "Back to Daily Agent reports" }).click();
  await expect(page).not.toHaveURL(/asrDailyReport=2026-05-14/);
  await expect(
    taskPage.getByRole("tab", { name: "Daily Agent Records", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await taskPage.getByRole("tab", { name: /Files/ }).click();
  await page.getByRole("button", { name: "Open transcript" }).first().click();
  await expect(page).toHaveURL(/asrFile=file-1/);
  const filePage = page.getByTestId("asr-task-file-detail-page");
  await expect(filePage).toBeVisible();
  await expect(filePage.getByText("Original Audio")).toBeVisible();
  const audio = filePage.locator('audio[aria-label="Original audio player"]');
  await expect(audio).toBeVisible();
  await expect(audio).toHaveAttribute("src", /\/asr\/tasks\/task-1\/files\/file-1\/source/);
  await audio.evaluate((element) => {
    Object.defineProperty(element, "currentTime", {
      value: 0,
      writable: true,
      configurable: true,
    });
    (element as HTMLAudioElement).play = () => Promise.resolve();
  });
  await expect(filePage.getByText("File Timeline")).toBeVisible();
  await expect(filePage.getByText("Full Transcript")).toBeVisible();
  await expect(filePage.getByText("ffprobe.date_creation_time")).toBeVisible();
  await filePage.getByRole("button", { name: "00:00:01.000" }).click();
  await expect(filePage.getByText("时间线文本")).toHaveCount(2);
  await expect(filePage.getByText("第二段文本")).toHaveCount(2);
  await expect(filePage.getByTestId("asr-transcript-segment-1")).toHaveAttribute(
    "aria-current",
    "true",
  );
  const jumpedMs = await audio.evaluate((element) =>
    Math.round((element as HTMLAudioElement).currentTime * 1000),
  );
  expect(jumpedMs).toBe(1000);

  const segments = filePage.getByTestId("asr-transcript-segments");
  await page.waitForTimeout(150);
  await segments.evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: -120 }));
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await audio.evaluate((element) => {
    (element as HTMLAudioElement).currentTime = 10;
    element.dispatchEvent(new Event("timeupdate", { bubbles: true }));
  });
  await expect(filePage.getByTestId("asr-transcript-segment-10")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await page.waitForTimeout(300);
  expect(await segments.evaluate((element) => element.scrollTop)).toBe(0);

  await audio.evaluate((element) => {
    (element as HTMLAudioElement).currentTime = 10;
    element.dispatchEvent(new Event("seeking", { bubbles: true }));
    element.dispatchEvent(new Event("seeked", { bubbles: true }));
  });
  await expect(filePage.getByTestId("asr-transcript-segment-10")).toHaveAttribute(
    "aria-current",
    "true",
  );
  if (await segments.evaluate((element) => element.scrollHeight > element.clientHeight)) {
    await expect
      .poll(async () => segments.evaluate((element) => element.scrollTop), { timeout: 1000 })
      .toBeGreaterThan(0);
  }

  await page.waitForTimeout(150);
  await segments.evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new WheelEvent("wheel", { bubbles: true, deltaY: -120 }));
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await audio.evaluate((element) => {
    (element as HTMLAudioElement).currentTime = 10;
    element.dispatchEvent(new Event("timeupdate", { bubbles: true }));
  });
  await expect(filePage.getByTestId("asr-transcript-segment-10")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await page.waitForTimeout(300);
  expect(await segments.evaluate((element) => element.scrollTop)).toBe(0);
  if (await segments.evaluate((element) => element.scrollHeight > element.clientHeight)) {
    await expect
      .poll(async () => segments.evaluate((element) => element.scrollTop), { timeout: 8000 })
      .toBeGreaterThan(0);
  }

  await audio.evaluate((element) => {
    (element as HTMLAudioElement).currentTime = 5;
    element.dispatchEvent(new Event("seeked", { bubbles: true }));
  });
  await expect(filePage.getByTestId("asr-transcript-segment-5")).toHaveAttribute(
    "aria-current",
    "true",
  );
  await page.getByRole("button", { name: "Back to task files" }).click();
  await expect(page).not.toHaveURL(/asrFile=file-1/);
  await page.getByRole("button", { name: "Back to directory tasks" }).click();
  await expect(page).not.toHaveURL(/asrTask=task-1/);
  await expect(page.getByText("Directory Tasks")).toBeVisible();

  await page
    .getByTestId("ai-section-content")
    .getByRole("button", { name: "Run" })
    .click();
  await expect(page.getByText("ASR task started")).toBeVisible();
});

test("ASR task detail can queue bulk retry for all failed chunks", async ({ page }) => {
  await installAsrMicrophoneMocks(page);
  let bulkRetryRequested = 0;
  let bulkRetryState: Record<string, unknown> | undefined;

  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        tasks: [
          {
            id: "task-bulk",
            name: "Bulk retry task",
            audio_dir: "/tmp/asr-audio",
            recursive: true,
            enabled: true,
            schedule: { kind: "daily", hour: 2, minute: 0 },
            language: "chinese",
            model: "Qwen3-ASR-1.7B",
            runtime_strategy: "reuse_per_file",
            created_at_ms: Date.now(),
            updated_at_ms: Date.now(),
            summary: {
              discovered: 2,
              processed: 2,
              pending: 0,
              failed: 0,
              partial_success: 2,
              failed_chunk_count: 3,
              deleted_after_processing: 0,
              running: false,
            },
          },
        ],
      }),
    });
  });

  await page.route("**/_bifrost/api/asr/tasks/task-bulk/retry-failed-chunks", async (route) => {
    bulkRetryRequested += 1;
    bulkRetryState = {
      task_id: "task-bulk",
      status: "queued",
      queued_files: 2,
      processed_files: 0,
      total_failed_chunks: 3,
      recovered_chunks: 0,
      still_failed_chunks: 0,
      updated_at_ms: Date.now(),
      message: "Queued 2 files with 3 failed chunks for retry",
      results: [],
    };
    await route.fulfill({
      status: 202,
      contentType: "application/json",
      body: JSON.stringify({
        message: "Queued 2 files with 3 failed chunks for retry",
        retry: bulkRetryState,
      }),
    });
  });

  await page.route("**/_bifrost/api/asr/tasks/task-bulk", async (route) => {
    if (route.request().method() !== "GET") {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: "task-bulk",
        name: "Bulk retry task",
        audio_dir: "/tmp/asr-audio",
        recursive: true,
        enabled: true,
        schedule: { kind: "daily", hour: 2, minute: 0 },
        language: "chinese",
        model: "Qwen3-ASR-1.7B",
        runtime_strategy: "reuse_per_file",
        created_at_ms: Date.now(),
        updated_at_ms: Date.now(),
        summary: {
          discovered: 2,
          processed: 2,
          pending: 0,
          failed: 0,
          partial_success: 2,
          failed_chunk_count: 3,
          deleted_after_processing: 0,
          running: false,
        },
        bulk_retry: bulkRetryState,
        daily_documents: [],
        files: [
          {
            key: "file-a",
            task_id: "task-bulk",
            source_path: "/tmp/asr-audio/a.wav",
            media_duration_ms: 60000,
            status: "partial_success",
            text_chars: 128,
            failed_chunks: [
              {
                chunk_index: 1,
                offset_secs: 28,
                duration_secs: 30,
                error: "asr-server disconnected",
                attempts: 3,
              },
            ],
          },
          {
            key: "file-b",
            task_id: "task-bulk",
            source_path: "/tmp/asr-audio/b.wav",
            media_duration_ms: 90000,
            status: "partial_success",
            text_chars: 256,
            failed_chunks: [
              {
                chunk_index: 2,
                offset_secs: 56,
                duration_secs: 30,
                error: "memory footprint limit",
                attempts: 3,
              },
              {
                chunk_index: 3,
                offset_secs: 84,
                duration_secs: 30,
                error: "memory footprint limit",
                attempts: 3,
              },
            ],
          },
        ],
      }),
    });
  });

  await openPage(page, "ai?aiSection=tools-asr&asrTask=task-bulk");
  const taskPage = page.getByTestId("asr-task-detail-page");
  await expect(taskPage.getByText("Directory Task: Bulk retry task")).toBeVisible();
  await expect(taskPage.getByText("(3 failed chunks)")).toBeVisible();

  await taskPage.getByRole("button", { name: "Retry all failed chunks" }).click();
  await page.getByRole("button", { name: "OK" }).click();
  await expect.poll(() => bulkRetryRequested).toBe(1);
  await expect(page.getByTestId("asr-bulk-retry-status")).toContainText(
    "Bulk chunk retry",
  );
  await expect(page.getByTestId("asr-bulk-retry-status")).toContainText("queued");
  await expect(page.getByTestId("asr-bulk-retry-status")).toContainText("0/2 files");
  await expect(page.getByTestId("asr-bulk-retry-status")).toContainText(
    "0/3 chunks recovered",
  );
});

test("ASR task detail tolerates older responses without daily documents", async ({ page }) => {
  await installAsrMicrophoneMocks(page);
  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        tasks: [
          {
            id: "legacy-task",
            name: "Legacy task",
            audio_dir: "/tmp/asr-audio",
            recursive: true,
            enabled: true,
            schedule: { kind: "daily", hour: 2, minute: 0 },
            language: "chinese",
            model: "Qwen3-ASR-1.7B",
            created_at_ms: Date.now(),
            updated_at_ms: Date.now(),
            next_run_at_ms: Date.now() + 60000,
            summary: {
              discovered: 0,
              processed: 0,
              pending: 0,
              failed: 0,
              deleted_after_processing: 0,
              running: false,
            },
          },
        ],
      }),
    });
  });
  await page.route("**/_bifrost/api/asr/tasks/legacy-task", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: "legacy-task",
        name: "Legacy task",
        audio_dir: "/tmp/asr-audio",
        recursive: true,
        enabled: true,
        schedule: { kind: "daily", hour: 2, minute: 0 },
        language: "chinese",
        model: "Qwen3-ASR-1.7B",
        created_at_ms: Date.now(),
        updated_at_ms: Date.now(),
        last_run_at_ms: Date.now() - 30000,
        next_run_at_ms: Date.now() + 86400000,
        summary: {
          discovered: 0,
          processed: 0,
          pending: 0,
          failed: 0,
          deleted_after_processing: 0,
          running: false,
        },
        files: [],
      }),
    });
  });

  await openPage(page, "ai?aiSection=tools-asr&asrTask=legacy-task");
  const taskPage = page.getByTestId("asr-task-detail-page");
  await expect(taskPage).toBeVisible();
  await expect(taskPage.getByText("Directory Task: Legacy task")).toBeVisible();
  await expect(taskPage.getByRole("tab", { name: "Daily Docs (0)" })).toBeVisible();
  await taskPage.getByRole("tab", { name: "Daily Docs (0)" }).click();
  await expect(
    taskPage.getByText("Daily documents are available after the ASR backend is updated"),
  ).toBeVisible();
});
