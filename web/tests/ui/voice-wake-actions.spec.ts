import { expect, type Page, test } from "@playwright/test";
import { openPage } from "./helpers/admin-helpers";

const DEFAULT_SPEAKER_THRESHOLD = 0.6;

interface WakeProfile {
  id: string;
  display_name: string;
  voiceprint_profile_id: string | null;
  speaker_threshold: number;
  created_at_ms: number;
  updated_at_ms: number;
}

interface WakeBinding {
  id: string;
  enabled: boolean;
  phrase: string;
  normalized_phrase: string;
  profile_id: string;
  kws_score: number;
  kws_threshold: number;
  speaker_threshold: number;
  cooldown_ms: number;
  action: {
    type: "key_press";
    key: string | null;
    keycode: number | null;
    modifiers: string[];
    press_count: number;
  };
  created_at_ms: number;
  updated_at_ms: number;
  last_triggered_at_ms: number | null;
}

interface VoiceWakeMockOptions {
  speakerProfiles?: boolean;
  savedBinding?: boolean;
}

async function installVoiceWakeMocks(page: Page, options: VoiceWakeMockOptions = {}) {
  const withSpeakerProfiles = options.speakerProfiles ?? true;
  const withSavedBinding = options.savedBinding ?? true;
  const profiles: WakeProfile[] = [];
  const speakerProfiles = withSpeakerProfiles
    ? [
        {
          id: "spk_eden",
          display_name: "Eden",
          source: "enrollment",
          diarization_profile: "sherpa-onnx-balanced",
          embedding_model: "sherpa-onnx",
          embedding_dim: 16,
          template_count: 1,
          prototype_count: 1,
          total_duration_ms: 3200,
          created_at_ms: Date.now(),
          updated_at_ms: Date.now(),
        },
      ]
    : [];
  const bindings: WakeBinding[] = [];
  const events: Array<Record<string, unknown>> = [];
  const listenerStartBodies: Array<Record<string, unknown>> = [];
  const now = Date.now();
  if (withSpeakerProfiles && withSavedBinding) {
    profiles.push({
      id: "wake_profile_recorded",
      display_name: "Eden",
      voiceprint_profile_id: "spk_eden",
      speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
      created_at_ms: now,
      updated_at_ms: now,
    });
    bindings.push({
      id: "wake_binding_recorded",
      enabled: true,
      phrase: "打开录音",
      normalized_phrase: "打开录音",
      profile_id: "wake_profile_recorded",
      kws_score: 1.5,
      kws_threshold: 0.35,
      speaker_threshold: DEFAULT_SPEAKER_THRESHOLD,
      cooldown_ms: 1500,
      action: {
        type: "key_press",
        key: "space",
        keycode: null,
        modifiers: ["cmd"],
        press_count: 1,
      },
      created_at_ms: now,
      updated_at_ms: now,
      last_triggered_at_ms: null,
    });
  }
  const listener = {
    running: false,
    source: "mic",
    device: null as string | null,
    worker_pid: null as number | null,
    chunk_ms: 2500,
    started_at_ms: null as number | null,
    stopped_at_ms: null as number | null,
    last_transcript: null as string | null,
    last_transcript_at_ms: null as number | null,
    last_error: null as string | null,
    last_error_at_ms: null as number | null,
    last_speaker_profile_id: null as string | null,
    last_speaker_confidence: null as number | null,
    last_speaker_status: null as string | null,
    trigger_count: 0,
  };

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
        install_dir: "/tmp/bifrost-asr",
        model_dir: "/tmp/bifrost-asr/Qwen3-ASR-0.6B",
        model: "Qwen3-ASR-0.6B",
        language: "chinese",
        owner_module: "model_management",
        message: "ready",
      },
    });
  });
  await page.route("**/_bifrost/api/asr/service/start**", async (route) => {
    await route.fulfill({
      json: {
        ready: true,
        managed: true,
        server_url: "http://127.0.0.1:54321",
        message: "ASR service ready",
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
    await route.fulfill({ json: { profiles: speakerProfiles } });
  });
  await page.route("**/_bifrost/api/voice/wake/status", async (route) => {
    await route.fulfill({
      json: {
        enabled: true,
        profile_count: profiles.length,
        binding_count: bindings.length,
        event_count: events.length,
        mode: "backend_asr_listener",
        store_path: "/tmp/bifrost/voice/wake/actions.json",
        default_dry_run: true,
        listener,
      },
    });
  });
  await page.route("**/_bifrost/api/voice/wake/profiles", async (route) => {
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON() as {
        display_name: string;
        voiceprint_profile_id: string;
        speaker_threshold?: number;
      };
      const now = Date.now();
      const profile = {
        id: `wake_profile_${profiles.length + 1}`,
        display_name: body.display_name,
        voiceprint_profile_id: body.voiceprint_profile_id,
        speaker_threshold: body.speaker_threshold ?? DEFAULT_SPEAKER_THRESHOLD,
        created_at_ms: now,
        updated_at_ms: now,
      };
      profiles.push(profile);
      await route.fulfill({ status: 201, json: profile });
      return;
    }
    await route.fulfill({ json: { profiles } });
  });
  await page.route("**/_bifrost/api/voice/wake/bindings", async (route) => {
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON() as {
        phrase: string;
        profile_id: string;
        cooldown_ms?: number;
        speaker_threshold?: number;
        action: WakeBinding["action"];
      };
      const now = Date.now();
      const binding = {
        id: `wake_binding_${bindings.length + 1}`,
        enabled: true,
        phrase: body.phrase,
        normalized_phrase: body.phrase,
        profile_id: body.profile_id,
        kws_score: 1.5,
        kws_threshold: 0.35,
        speaker_threshold: body.speaker_threshold ?? DEFAULT_SPEAKER_THRESHOLD,
        cooldown_ms: body.cooldown_ms ?? 1500,
        action: body.action,
        created_at_ms: now,
        updated_at_ms: now,
        last_triggered_at_ms: null,
      };
      bindings.push(binding);
      await route.fulfill({ status: 201, json: binding });
      return;
    }
    await route.fulfill({ json: { bindings } });
  });
  await page.route("**/_bifrost/api/voice/wake/listener/start", async (route) => {
    listenerStartBodies.push(route.request().postDataJSON() as Record<string, unknown>);
    if (!bindings.length) {
      await route.fulfill({
        status: 400,
        body: "voice wake listener requires a saved voice command binding before starting",
      });
      return;
    }
    const binding = bindings[bindings.length - 1]!;
    const event = {
      id: `wake_event_${events.length + 1}`,
      binding_id: binding.id,
      phrase: binding.phrase,
      profile_id: binding.profile_id,
      speaker_confidence: 0.91,
      dry_run: false,
      matched_at_ms: Date.now(),
      action_result: {
        action_type: "key_press",
        dry_run: false,
        executed: true,
        platform: "macos",
        message: "key press executed",
        command_preview: "tell application \"System Events\" to key code 49",
      },
    };
    events.push(event);
    binding.last_triggered_at_ms = Number(event.matched_at_ms);
    listener.running = true;
    listener.worker_pid = 4242;
    listener.started_at_ms = Date.now();
    listener.last_transcript = `现在${binding.phrase}`;
    listener.last_transcript_at_ms = Date.now();
    listener.last_speaker_profile_id = "spk_eden";
    listener.last_speaker_confidence = 0.91;
    listener.last_speaker_status = "matched";
    listener.trigger_count += 1;
    await route.fulfill({ json: { listener } });
  });
  await page.route("**/_bifrost/api/voice/wake/listener/stop", async (route) => {
    listener.running = false;
    listener.worker_pid = null;
    listener.stopped_at_ms = Date.now();
    await route.fulfill({
      json: { listener },
    });
  });
  await page.route("**/_bifrost/api/voice/wake/events", async (route) => {
    await route.fulfill({ json: { events } });
  });

  return { listenerStartBodies };
}

test("ASR 页面提供录音、快捷键绑定和真实监听触发入口", async ({ page }) => {
  const mocks = await installVoiceWakeMocks(page);
  await openPage(page, "ai?aiSection=tools-asr&asrTab=voice");

  await expect(page.getByTestId("voice-wake-actions-card")).toBeVisible();
  await expect(page.getByTestId("voice-wake-record-button")).toBeVisible();
  await expect(page.getByTestId("voice-wake-phrase-input")).toHaveValue("打开录音");
  await expect(page.getByTestId("voice-wake-voiceprint-select")).toBeVisible();
  await expect(page.getByTestId("voice-wake-voiceprint-select")).toContainText("Eden");
  const shortcutInput = page.getByTestId("voice-wake-shortcut-input");
  await expect(shortcutInput).toBeVisible();
  await expect(shortcutInput).toHaveValue("cmd+space");
  await shortcutInput.click();
  await shortcutInput.press("Alt+Shift+A");
  await expect(shortcutInput).toHaveValue("shift+option+a");
  const recordBox = await page.getByTestId("voice-wake-record-button").boundingBox();
  const phraseBox = await page.getByTestId("voice-wake-phrase-input").boundingBox();
  expect(recordBox).not.toBeNull();
  expect(phraseBox).not.toBeNull();
  expect(phraseBox!.y).toBeGreaterThan(recordBox!.y);
  expect(Math.abs(phraseBox!.x - recordBox!.x)).toBeLessThan(32);

  await page.getByTestId("voice-wake-save-action").click();
  await expect(page.getByTestId("voice-wake-actions-card")).toContainText("shift+option+a");
  await page.getByTestId("voice-wake-listener-switch").click();
  await expect(page.getByTestId("voice-wake-listening-status")).toContainText("Backend Listening");
  await expect(page.getByTestId("voice-wake-listening-status")).toContainText("现在打开录音");
  await expect(page.getByTestId("voice-wake-listening-status")).toContainText("91% voice");
  await expect(page.getByTestId("voice-wake-event-table")).toContainText("打开录音");
  await expect(page.getByTestId("voice-wake-event-table")).toContainText("shift+option+a");
  await expect(page.getByTestId("voice-wake-event-table")).toContainText("Executed");
  expect(mocks.listenerStartBodies).toHaveLength(1);
});

test("Voice Wake 开关在没有声纹时禁用并展示提示", async ({ page }) => {
  const mocks = await installVoiceWakeMocks(page, { speakerProfiles: false, savedBinding: false });
  await openPage(page, "ai?aiSection=tools-asr&asrTab=voice");

  await expect(page.getByTestId("voice-wake-listener-switch")).toBeDisabled();
  await expect(page.getByTestId("voice-wake-start-listening")).toBeDisabled();
  await expect(page.getByTestId("voice-wake-listener-block-reason")).toContainText(
    "Record wake audio",
  );
  expect(mocks.listenerStartBodies).toHaveLength(0);
});

test("Voice Wake 开关在没有保存指令时禁用并展示提示", async ({ page }) => {
  const mocks = await installVoiceWakeMocks(page, { speakerProfiles: true, savedBinding: false });
  await openPage(page, "ai?aiSection=tools-asr&asrTab=voice");

  await expect(page.getByTestId("voice-wake-listener-switch")).toBeDisabled();
  await expect(page.getByTestId("voice-wake-start-listening")).toBeDisabled();
  await expect(page.getByTestId("voice-wake-listener-block-reason")).toContainText(
    "save the voice command",
  );
  expect(mocks.listenerStartBodies).toHaveLength(0);
});
