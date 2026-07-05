import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  checkVersion as checkVersionApi,
  getUpgradeProgress as getUpgradeProgressApi,
  startUpgrade as startUpgradeApi,
  type UpgradeChannel,
} from "../api/version";
import type { UpgradePhase, UpgradeProgress, VersionCheckResponse } from "../types";
import { isConnectionIssueError } from "../api/client";
import { isDesktopShell } from "../runtime";

const SEEN_VERSIONS_STORAGE_KEY = "bifrost-seen-versions";
const CHECK_INTERVAL_MS = 60 * 60 * 1000;
export const DESKTOP_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const UPGRADE_POLL_INTERVAL_MS = 1000;
/** sessionStorage flag guarding the single post-upgrade auto-reload. */
export const UPGRADE_RELOAD_PENDING_KEY = "bifrost-upgrade-reload-pending";

interface CheckVersionOptions {
  forceRefresh?: boolean;
  skipCache?: boolean;
}

interface VersionState {
  hasUpdate: boolean;
  currentVersion: string;
  latestVersion: string | null;
  releaseHighlights: string[];
  releaseUrl: string | null;
  loading: boolean;
  lastChecked: number | null;
  seenVersions: string[];
  modalVisible: boolean;

  // Upgrade flow state.
  upgradePhase: UpgradePhase;
  upgradePercent: number | null;
  upgradeMessage: string;
  upgradeError: string | null;
  upgrading: boolean;

  checkVersion: (options?: CheckVersionOptions) => Promise<void>;
  markVersionSeen: (version: string) => void;
  isVersionSeen: (version: string) => boolean;
  setModalVisible: (visible: boolean) => void;
  shouldShowAutoModal: () => boolean;
  startUpgrade: () => Promise<void>;
  pollUpgradeProgress: () => void;
  stopPollUpgradeProgress: () => void;
}

/** True while an upgrade is mid-flight (non-terminal). */
function isActivePhase(phase: UpgradePhase): boolean {
  return (
    phase === "checking" ||
    phase === "downloading" ||
    phase === "installing" ||
    phase === "restarting"
  );
}

function currentUpgradeChannel(): UpgradeChannel {
  return isDesktopShell() ? "desktop" : "cli";
}

let pollTimer: ReturnType<typeof setInterval> | null = null;
/** Set once we observe the backend drop (proxy restart) during an upgrade. */
let sawDisconnect = false;

/** Reload the page exactly once after an upgrade, guarded by sessionStorage. */
function triggerUpgradeReloadOnce() {
  try {
    if (sessionStorage.getItem(UPGRADE_RELOAD_PENDING_KEY) === "done") {
      return;
    }
    sessionStorage.setItem(UPGRADE_RELOAD_PENDING_KEY, "done");
  } catch {
    // sessionStorage unavailable — fall through and reload anyway.
  }
  if (isDesktopShell()) {
    void import("../desktop/tauri")
      .then(({ restartDesktopAfterUpdate }) => restartDesktopAfterUpdate())
      .catch(() => window.location.reload());
    return;
  }
  window.location.reload();
}

export const useVersionStore = create<VersionState>()(
  persist(
    (set, get) => ({
      hasUpdate: false,
      currentVersion: "",
      latestVersion: null,
      releaseHighlights: [],
      releaseUrl: null,
      loading: false,
      lastChecked: null,
      seenVersions: [],
      modalVisible: false,

      upgradePhase: "idle",
      upgradePercent: null,
      upgradeMessage: "",
      upgradeError: null,
      upgrading: false,

      checkVersion: async (options: CheckVersionOptions = {}) => {
        const { forceRefresh = false, skipCache = false } = options;
        const state = get();

        if (!forceRefresh && !skipCache && state.lastChecked) {
          const elapsed = Date.now() - state.lastChecked;
          const interval = currentUpgradeChannel() === "desktop"
            ? DESKTOP_CHECK_INTERVAL_MS
            : CHECK_INTERVAL_MS;
          if (elapsed < interval) {
            return;
          }
        }

        set({ loading: true });

        try {
          const response: VersionCheckResponse = await checkVersionApi(
            forceRefresh,
            currentUpgradeChannel(),
          );
          set({
            hasUpdate: response.has_update,
            currentVersion: response.current_version,
            latestVersion: response.latest_version,
            releaseHighlights: response.release_highlights,
            releaseUrl: response.release_url,
            lastChecked: Date.now(),
            loading: false,
          });
        } catch (error) {
          if (!isConnectionIssueError(error)) {
            console.error("Failed to check version:", error);
          }
          set({ loading: false });
        }
      },

      markVersionSeen: (version: string) => {
        const state = get();
        if (!state.seenVersions.includes(version)) {
          set({ seenVersions: [...state.seenVersions, version] });
        }
      },

      isVersionSeen: (version: string) => {
        return get().seenVersions.includes(version);
      },

      setModalVisible: (visible: boolean) => {
        set({ modalVisible: visible });
      },

      shouldShowAutoModal: () => {
        const state = get();
        if (!state.hasUpdate || !state.latestVersion) {
          return false;
        }
        return !state.seenVersions.includes(state.latestVersion);
      },

      startUpgrade: async () => {
        if (get().upgrading) {
          return;
        }
        sawDisconnect = false;
        try {
          sessionStorage.removeItem(UPGRADE_RELOAD_PENDING_KEY);
        } catch {
          // ignore — sessionStorage may be unavailable.
        }
        set({
          upgrading: true,
          upgradePhase: "checking",
          upgradePercent: null,
          upgradeMessage: "Checking for updates…",
          upgradeError: null,
        });
        try {
          const progress = await startUpgradeApi(currentUpgradeChannel());
          set({
            upgradePhase: progress.phase,
            upgradePercent: progress.percent,
            upgradeMessage: progress.message,
            upgradeError: progress.error,
          });
          get().pollUpgradeProgress();
        } catch (error) {
          set({
            upgrading: false,
            upgradePhase: "failed",
            upgradeError:
              error instanceof Error ? error.message : "Failed to start upgrade",
          });
        }
      },

      pollUpgradeProgress: () => {
        if (pollTimer) {
          return;
        }
        pollTimer = setInterval(async () => {
          try {
            const previousPhase = get().upgradePhase;
            const progress: UpgradeProgress = await getUpgradeProgressApi();
            set({
              upgradePhase: progress.phase,
              upgradePercent: progress.percent,
              upgradeMessage: progress.message,
              upgradeError: progress.error,
            });

            if (progress.phase === "completed") {
              get().stopPollUpgradeProgress();
              set({ upgrading: false });
              triggerUpgradeReloadOnce();
            } else if (progress.phase === "failed") {
              get().stopPollUpgradeProgress();
              set({ upgrading: false });
            }
            if (progress.phase === "idle" && isActivePhase(previousPhase)) {
              get().stopPollUpgradeProgress();
              set({
                upgrading: false,
                upgradePercent: null,
                upgradeMessage: "",
                upgradeError: null,
              });
              if (sawDisconnect) {
                triggerUpgradeReloadOnce();
              } else {
                void get().checkVersion({ forceRefresh: true, skipCache: true });
              }
            }
          } catch (error) {
            // A connection drop during Restarting is expected (proxy restart).
            // Remember it: once the backend comes back and we read a terminal
            // state, we trigger the single auto-reload.
            if (isConnectionIssueError(error) && isActivePhase(get().upgradePhase)) {
              sawDisconnect = true;
            }
            if (sawDisconnect && !isConnectionIssueError(error)) {
              // Reconnected after a drop — refresh once to load the new build.
              get().stopPollUpgradeProgress();
              set({ upgrading: false });
              triggerUpgradeReloadOnce();
            }
          }
        }, UPGRADE_POLL_INTERVAL_MS);
      },

      stopPollUpgradeProgress: () => {
        if (pollTimer) {
          clearInterval(pollTimer);
          pollTimer = null;
        }
      },
    }),
    {
      name: SEEN_VERSIONS_STORAGE_KEY,
      partialize: (state) => ({
        seenVersions: state.seenVersions,
        lastChecked: state.lastChecked,
      }),
    },
  ),
);
