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
        window.__asrStoppedTracks = 0;
        window.__asrClosedAudioContexts = 0;
        window.__asrRecorderIntervals = [];

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
          createAnalyser() {
            return new FakeAnalyser();
          }

          createMediaStreamSource() {
            return { connect() {} };
          }

          close() {
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

        class FakeMediaRecorder {
          constructor() {
            this.state = "inactive";
            this.mimeType = "audio/webm";
          }

          start(intervalMs) {
            this.state = "recording";
            window.__asrRecorderIntervals.push(intervalMs);
            this._timer = window.setInterval(() => {
              this.ondataavailable?.({
                data: new Blob(["asr-audio"], { type: "audio/webm" }),
              });
            }, Math.min(intervalMs || 1000, 200));
          }

          stop() {
            if (this.state !== "recording") {
              return;
            }
            this.state = "inactive";
            window.clearInterval(this._timer);
            this.onstop?.();
          }
        }

        class FakeWebSocket {
          static CONNECTING = 0;
          static OPEN = 1;
          static CLOSING = 2;
          static CLOSED = 3;

          constructor() {
            this.readyState = FakeWebSocket.CONNECTING;
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
                this._emit({
                  type: "stream",
                  phase: "stream",
                  status: "ok",
                  progress: 50,
                  message: "mock stream window",
                  detail: "processed_ms=1000",
                });
                window.setTimeout(() => {
                  this._emit({
                    type: "partial",
                    index: 1,
                    start_ms: 0,
                    end_ms: 1000,
                    stable_start_ms: 0,
                    stable_end_ms: 800,
                    text: "测试",
                    delta: "测试",
                    committed: "",
                  });
                }, 120);
              } else if (message.type === "finish") {
                this._emit({
                  type: "final",
                  index: 1,
                  start_ms: 0,
                  end_ms: 1000,
                  stable_start_ms: 0,
                  stable_end_ms: 1000,
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

            this._emit({
              type: "stream",
              phase: "stream",
              status: "ok",
              progress: 55,
              message: "mock binary audio received",
              detail: "processed_ms=4000",
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

        Object.defineProperty(window, "MediaRecorder", {
          value: FakeMediaRecorder,
          configurable: true,
        });
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
  await expect(page.getByText("partial[1]: 0-800ms")).toBeVisible();

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
    recorderIntervals: (
      window as Window & { __asrRecorderIntervals: number[] }
    ).__asrRecorderIntervals,
  }));
  expect(cleanupCounts.stoppedTracks).toBeGreaterThanOrEqual(2);
  expect(cleanupCounts.closedAudioContexts).toBeGreaterThanOrEqual(2);
  expect(cleanupCounts.recorderIntervals).toContain(1000);

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
            deleted_after_processing: 0,
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
                  deleted_after_processing: 1,
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
            deleted_after_processing: 1,
            running: false,
          },
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
          deleted_after_processing: 1,
          running: false,
        },
        files: [
          {
            key: "file-1",
            task_id: "task-1",
            source_path: "/tmp/asr-audio/sample.wav",
            source_size: 1234,
            source_modified_ms: Date.now() - 60000,
            source_created_at_ms: new Date("2026-05-14T11:44:33+08:00").getTime(),
            source_created_at_source: "ffprobe.date_creation_time",
            media_duration_ms: 2000,
            status: "success",
            output_text_path: "/tmp/bifrost/asr/data/text/task-1/file-1.txt",
            output_metadata_path: "/tmp/bifrost/asr/data/text/task-1/file-1.json",
            output_timeline_path: "/tmp/bifrost/asr/data/text/task-1/file-1.timeline.json",
            text_chars: 4,
            finished_at_ms: Date.now() - 30000,
          },
        ],
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
        media_duration_ms: 2000,
        model: "Qwen3-ASR-1.7B",
        language: "chinese",
        processed_at_ms: Date.now(),
        segments: [
          {
            index: 0,
            audio_start_ms: 0,
            audio_end_ms: 2000,
            absolute_start_ms: new Date("2026-05-14T11:44:33+08:00").getTime(),
            absolute_end_ms: new Date("2026-05-14T11:44:35+08:00").getTime(),
            text: "时间线文本",
          },
        ],
      }),
    });
  });

  await openPage(page, "ai?aiSection=tools-asr");
  await expect(page.getByText("Directory Tasks")).toBeVisible();
  await page.getByPlaceholder("Meeting audio watcher").fill("Recordings");
  await page.getByPlaceholder("/Users/eden/Recordings").fill("/tmp/asr-audio");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.getByText("Recordings")).toBeVisible();
  await expect(page.getByText("/tmp/asr-audio")).toBeVisible();
  await expect(page.getByText("Daily at 02:00")).toBeVisible();
  await expect(page.getByText(/processed 1, pending 0/)).toBeVisible();
  await expect(page.getByText(/deleted after processing 1/)).toBeVisible();
  await page.getByRole("button", { name: "View details" }).click();
  const taskDialog = page.getByRole("dialog", { name: "Directory Task: Recordings" });
  await expect(taskDialog).toBeVisible();
  await expect(taskDialog.getByText("/tmp/asr-audio/sample.wav")).toHaveCount(2);
  await expect(taskDialog.getByText("/tmp/bifrost/asr/data/text/task-1/file-1.txt")).toBeVisible();
  await expect(taskDialog.getByText("success")).toBeVisible();
  await expect(taskDialog.getByText("File Timeline")).toBeVisible();
  await expect(taskDialog.getByText("Full Transcript")).toBeVisible();
  await page.getByRole("button", { name: "Open timeline" }).click();
  await expect(taskDialog.getByText("ffprobe.date_creation_time").last()).toBeVisible();
  await expect(taskDialog.getByText("00:00:00.000 - 00:00:02.000")).toBeVisible();
  await expect(taskDialog.getByText("时间线文本")).toHaveCount(2);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Directory Task: Recordings" })).toBeHidden();

  await page
    .getByTestId("ai-section-content")
    .getByRole("button", { name: "Run" })
    .click();
  await expect(page.getByText(/Processed 1, failed 0/)).toBeVisible();
});
