import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  checkVersion as checkVersionApi,
  getUpgradeProgress as getUpgradeProgressApi,
  startUpgrade as startUpgradeApi,
  type UpgradeChannel,
} from "../api/version";
import type { UpgradePhase, UpgradeProgress, VersionCheckResponse } from "../types";
import { isConnectionIssueError, normalizeApiErrorMessage } from "../api/client";
import { isDesktopShell } from "../runtime";

const SEEN_VERSIONS_STORAGE_KEY = "bifrost-seen-versions";
const CHECK_INTERVAL_MS = 60 * 60 * 1000;
export const DESKTOP_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const UPGRADE_POLL_INTERVAL_MS = 1000;
/** sessionStorage flag guarding the single post-upgrade auto-reload. */
export const UPGRADE_RELOAD_PENDING_KEY = "bifrost-upgrade-reload-pending";
const DESKTOP_RESTART_HANDOFF_FAILED_MESSAGE =
  "Desktop update installed, but restart handoff failed";

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
  resumeUpgradeProgress: () => Promise<boolean>;
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
function triggerUpgradeReloadOnce(desktopHandoff = false) {
  try {
    if (sessionStorage.getItem(UPGRADE_RELOAD_PENDING_KEY) === "done") {
      return;
    }
    sessionStorage.setItem(UPGRADE_RELOAD_PENDING_KEY, "done");
  } catch {
    // sessionStorage unavailable — fall through and reload anyway.
  }
  if (desktopHandoff && isDesktopShell()) {
    void import("../desktop/tauri")
      .then(({ restartDesktopAfterUpdate }) => restartDesktopAfterUpdate())
      .catch((error) => {
        try {
          sessionStorage.removeItem(UPGRADE_RELOAD_PENDING_KEY);
        } catch {
          // ignore — sessionStorage may be unavailable.
        }
        useVersionStore.setState({
          upgrading: false,
          upgradePhase: "failed",
          upgradeMessage: DESKTOP_RESTART_HANDOFF_FAILED_MESSAGE,
          upgradeError:
            error instanceof Error
              ? error.message
              : "Failed to restart the desktop app after update",
        });
      });
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
          const activeUpgrade = isActivePhase(get().upgradePhase);
          set({
            hasUpdate: activeUpgrade || response.has_update,
            currentVersion: response.current_version,
            latestVersion:
              activeUpgrade && get().latestVersion
                ? get().latestVersion
                : response.latest_version,
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
        const state = get();
        if (state.upgrading) {
          return;
        }
        if (
          isDesktopShell() &&
          state.upgradePhase === "failed" &&
          state.upgradeMessage === DESKTOP_RESTART_HANDOFF_FAILED_MESSAGE
        ) {
          set({
            upgrading: true,
            upgradePhase: "restarting",
            upgradeMessage: "Retrying desktop restart handoff…",
            upgradeError: null,
          });
          triggerUpgradeReloadOnce(true);
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
          if (await get().resumeUpgradeProgress()) {
            return;
          }
          set({
            upgrading: false,
            upgradePhase: "failed",
            upgradeError: normalizeApiErrorMessage(error, "Failed to start upgrade"),
          });
        }
      },

      resumeUpgradeProgress: async () => {
        try {
          const progress = await getUpgradeProgressApi();
          if (!isActivePhase(progress.phase)) {
            return false;
          }

          sawDisconnect = false;
          try {
            // A reload may have consumed an intermediate CLI-core transition.
            // The still-active transaction needs a fresh reload/handoff budget
            // for its eventual terminal desktop phase.
            sessionStorage.removeItem(UPGRADE_RELOAD_PENDING_KEY);
          } catch {
            // ignore — sessionStorage may be unavailable.
          }
          set({
            hasUpdate: true,
            latestVersion: progress.target_version ?? get().latestVersion,
            modalVisible: true,
            upgrading: true,
            upgradePhase: progress.phase,
            upgradePercent: progress.percent,
            upgradeMessage: progress.message,
            upgradeError: progress.error,
          });
          get().pollUpgradeProgress();
          return true;
        } catch (error) {
          if (!isConnectionIssueError(error)) {
            console.error("Failed to resume upgrade progress:", error);
          }
          return false;
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

            if (
              progress.phase === "restarting" &&
              progress.source === "desktop" &&
              isDesktopShell()
            ) {
              // App-owned installers must run only after the desktop shell and
              // bundled core have released their files. Stop polling before the
              // handoff exits this process; the relaunched core publishes the
              // terminal completed/failed state.
              get().stopPollUpgradeProgress();
              triggerUpgradeReloadOnce(true);
            } else if (progress.phase === "completed") {
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
