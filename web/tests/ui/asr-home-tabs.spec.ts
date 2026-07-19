import { expect, type Page, test } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

async function installAsrHomeTabMocks(page: Page) {
  await page.route("**/_bifrost/api/asr/capabilities", async (route) => {
    await route.fulfill({
      json: {
        platform: "macos",
        arch: "aarch64",
        supported_target: "macos-aarch64",
        qwen3_asr: { enabled: true, hidden: false, platform_supported: true },
        local_transcription: { enabled: true, hidden: false, platform_supported: true },
        speech_workbench: { enabled: true, hidden: false, platform_supported: true },
        directory_tasks: { enabled: true, hidden: false, platform_supported: true },
        speaker_diarization: { enabled: true, hidden: false, platform_supported: true },
        voiceprint: { enabled: true, hidden: false, platform_supported: true },
        voice_wake_asr: { enabled: true, hidden: false, platform_supported: true },
      },
    });
  });
  await page.route("**/_bifrost/api/asr/status**", async (route) => {
    await route.fulfill({
      json: {
        status: "ready",
        ready: true,
        installed: true,
        platform_supported: true,
        ffmpeg_available: true,
        managed: true,
        server_url: "http://127.0.0.1:54321",
        install_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs",
        model_dir: "/tmp/bifrost-asr-test/qwen3_asr_rs/Qwen3-ASR-0.6B",
        model: "Qwen3-ASR-0.6B",
        language: "chinese",
        owner_module: "model_management",
        message: "ready",
      },
    });
  });
  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    await route.fulfill({ json: { tasks: [] } });
  });
  await page.route("**/_bifrost/api/asr/external-volumes", async (route) => {
    await route.fulfill({ json: { volumes: [] } });
  });
  await page.route("**/_bifrost/api/speech/pipelines/status", async (route) => {
    await route.fulfill({
      json: {
        profiles: [],
        runtime: {
          platform_supported: true,
          asr_ready: true,
          diarization_ready: true,
          realtime_voice_active: false,
          offline_asr_active: false,
        },
        resources: {
          leases: [],
          realtime_voice_active: false,
          offline_asr_active: false,
          wake_listener_active: false,
        },
      },
    });
  });
  await page.route("**/_bifrost/api/asr/diarization/status**", async (route) => {
    await route.fulfill({
      json: {
        profile: {
          id: "sherpa-onnx-balanced",
          label: "Balanced",
          engine: "sherpa-onnx",
          quality_tier: "balanced",
          requires_init: false,
          ready: true,
          install_dir: "/tmp/bifrost-diarization",
          message: "ready",
        },
        voiceprint_dir: "/tmp/bifrost-voiceprints",
        speaker_profile_count: 0,
      },
    });
  });
  await page.route("**/_bifrost/api/asr/speaker-profiles", async (route) => {
    await route.fulfill({ json: { profiles: [] } });
  });
  await page.route("**/_bifrost/api/voice/wake/status", async (route) => {
    await route.fulfill({
      json: {
        enabled: true,
        profile_count: 0,
        binding_count: 0,
        event_count: 0,
        mode: "backend_asr_listener",
        store_path: "/tmp/bifrost/voice/wake/actions.json",
        default_dry_run: true,
        listener: {
          running: false,
          source: "mic",
          device: null,
          worker_pid: null,
          chunk_ms: 2500,
          started_at_ms: null,
          stopped_at_ms: null,
          last_transcript: null,
          last_transcript_at_ms: null,
          last_error: null,
          last_error_at_ms: null,
          last_speaker_profile_id: null,
          last_speaker_confidence: null,
          last_speaker_status: null,
          trigger_count: 0,
        },
      },
    });
  });
  await page.route("**/_bifrost/api/voice/wake/profiles", async (route) => {
    await route.fulfill({ json: { profiles: [] } });
  });
  await page.route("**/_bifrost/api/voice/wake/bindings", async (route) => {
    await route.fulfill({ json: { bindings: [] } });
  });
  await page.route("**/_bifrost/api/voice/wake/events", async (route) => {
    await route.fulfill({ json: { events: [] } });
  });
}

test("ASR 首页按定时任务、ASR 管理、声纹识别与唤醒三 Tab 分组", async ({
  page,
}) => {
  await installAsrHomeTabMocks(page);
  await openPage(page, "ai?aiSection=tools-asr");

  await expect(page.getByTestId("asr-home-tabs")).toBeVisible();
  await expect(page.getByRole("tab", { name: "Scheduled Tasks" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByTestId("asr-home-tab-scheduled")).toBeVisible();
  await expect(page.getByText("Directory Tasks")).toBeVisible();
  await expect(page.getByTestId("asr-workbench-card")).toHaveCount(0);
  await expect(page.getByTestId("asr-diarization-setup-card")).toHaveCount(0);
  await expect(page.getByTestId("voice-wake-actions-card")).toHaveCount(0);

  await page.getByRole("tab", { name: "ASR Management" }).click();
  await expect(page).toHaveURL(/asrTab=management/);
  await expect(page.getByTestId("asr-home-tab-management")).toBeVisible();
  await expect(page.getByText("Model Management", { exact: true })).toBeVisible();
  await expect(page.getByTestId("asr-workbench-card")).toBeVisible();
  await expect(page.getByTestId("asr-home-tab-scheduled")).toHaveCount(0);

  await page.getByRole("tab", { name: "Voiceprint & Wake" }).click();
  await expect(page).toHaveURL(/asrTab=voice/);
  await expect(page.getByTestId("asr-home-tab-voice")).toBeVisible();
  await expect(page.getByTestId("asr-diarization-setup-card")).toBeVisible();
  await expect(page.getByTestId("voice-wake-actions-card")).toBeVisible();
  await expect(page.getByTestId("asr-workbench-card")).toHaveCount(0);

  await page.reload();
  await expect(page.getByRole("tab", { name: "Voiceprint & Wake" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.getByTestId("voice-wake-actions-card")).toBeVisible();
});

test("ASR 目录任务按转录模式只展示实际生效的配置", async ({ page }) => {
  await installAsrHomeTabMocks(page);
  let submittedTask: Record<string, unknown> | undefined;
  await page.route("**/_bifrost/api/asr/tasks", async (route) => {
    if (route.request().method() === "POST") {
      submittedTask = route.request().postDataJSON() as Record<string, unknown>;
      await route.fulfill({ json: {} });
      return;
    }
    await route.fulfill({ json: { tasks: [] } });
  });
  await openPage(page, "ai?aiSection=tools-asr");

  await page
    .getByTestId("asr-home-tab-scheduled")
    .getByRole("button", { name: "New" })
    .click();
  await expect(page.getByRole("dialog", { name: "New Directory Task" })).toBeVisible();

  for (const label of [
    "Runtime",
    "File Concurrency",
    "Speaker Diarization",
    "Diarization Profile",
    "Known Speakers",
    "Voiceprint Matching",
    "Task Model",
    "Task Language",
  ]) {
    await expect(page.getByLabel(label, { exact: true })).toHaveCount(1);
  }
  await expect(page.getByLabel("MOSS Prompt", { exact: true })).toHaveCount(0);
  await page.getByRole("combobox", { name: "Task Model" }).click({ force: true });
  await page
    .locator(".ant-select-dropdown:visible .ant-select-item-option-content")
    .filter({ hasText: "Qwen3-ASR-1.7B" })
    .click();
  await expect(page.getByTestId("asr-task-model-select")).toContainText("Qwen3-ASR-1.7B");

  await page.getByRole("combobox", { name: "Transcription Mode" }).click({ force: true });
  await page
    .locator(".ant-select-dropdown:visible")
    .getByText("MOSS joint transcription (speaker-aware)", { exact: true })
    .click();

  await expect(page.getByLabel("MOSS Prompt", { exact: true })).toBeVisible();
  await page.getByLabel("MOSS Prompt", { exact: true }).fill("Preserve Bifrost terminology.");
  await expect(page.getByTestId("asr-transcription-prompt-count")).toHaveText("29 / 4000");
  for (const label of [
    "Runtime",
    "File Concurrency",
    "Speaker Diarization",
    "Diarization Profile",
    "Known Speakers",
    "Voiceprint Matching",
    "Task Model",
    "Task Language",
  ]) {
    await expect(page.getByLabel(label, { exact: true })).toHaveCount(0);
  }
  for (const label of ["Recursive", "Enabled", "External Devices"]) {
    await expect(page.getByLabel(label, { exact: true })).toHaveCount(1);
  }

  await page.getByRole("combobox", { name: "Transcription Mode" }).click({ force: true });
  await page
    .locator(".ant-select-dropdown:visible")
    .getByText("Standard ASR + speaker diarization", { exact: true })
    .click();

  await expect(page.getByLabel("MOSS Prompt", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Runtime", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Task Model", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Task Language", { exact: true })).toBeVisible();
  await expect(page.getByTestId("asr-task-model-select")).toContainText("Qwen3-ASR-1.7B");

  await page.getByRole("combobox", { name: "Transcription Mode" }).click({ force: true });
  await page
    .locator(".ant-select-dropdown:visible")
    .getByText("MOSS joint transcription (speaker-aware)", { exact: true })
    .click();
  await expect(page.getByLabel("MOSS Prompt", { exact: true })).toHaveValue(
    "Preserve Bifrost terminology.",
  );

  await page.getByLabel("Name", { exact: true }).fill("MOSS field visibility test");
  await page.getByLabel("Audio Directory", { exact: true }).fill("/tmp/moss-field-visibility");
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await expect.poll(() => submittedTask).toBeTruthy();
  expect(submittedTask).toMatchObject({
    name: "MOSS field visibility test",
    audio_dir: "/tmp/moss-field-visibility",
    model: "Qwen3-ASR-1.7B",
    language: "chinese",
    runtime_strategy: "reuse_per_file",
    max_concurrent_files: 1,
    transcription_mode: "moss_joint",
    transcription_prompt: "Preserve Bifrost terminology.",
    diarization: {
      enabled: true,
      profile: "sherpa-onnx-balanced",
      voiceprint_matching: true,
    },
  });
});

test("ASR 任务详情深链继续绕过首页 Tab", async ({ page }) => {
  await installAsrHomeTabMocks(page);
  await page.route("**/_bifrost/api/asr/tasks/task-tabs", async (route) => {
    await route.fulfill({
      json: {
        id: "task-tabs",
        name: "Tabs Detail Task",
        audio_dir: "/tmp/asr-tabs",
        recursive: true,
        enabled: false,
        schedule: { kind: "daily", hour: 2, minute: 0 },
        language: "chinese",
        model: "Qwen3-ASR-0.6B",
        runtime_strategy: "reuse_per_file",
        created_at_ms: Date.now(),
        updated_at_ms: Date.now(),
        summary: {
          discovered: 0,
          processed: 0,
          pending: 0,
          failed: 0,
          partial_success: 0,
          failed_chunk_count: 0,
          deleted_after_processing: 0,
          audio_source_bytes: 0,
          audio_source_file_count: 0,
          cleanable_source_bytes: 0,
          cleanable_source_file_count: 0,
          running: false,
        },
        files: [],
        daily_documents: [],
      },
    });
  });

  await openPage(page, "ai?aiSection=tools-asr&asrTask=task-tabs&asrTab=voice");

  await expect(page.getByTestId("asr-task-detail-page")).toBeVisible();
  await expect(page.getByText("Directory Task: Tabs Detail Task")).toBeVisible();
  await expect(page.getByTestId("asr-home-tabs")).toHaveCount(0);
});
