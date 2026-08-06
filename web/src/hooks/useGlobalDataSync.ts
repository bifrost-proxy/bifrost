import { useEffect, useRef } from 'react';
import { message } from 'antd';
import { useProxyStore } from '../stores/useProxyStore';
import { useFilterPanelStore } from '../stores/useFilterPanelStore';
import { useMetricsStore } from '../stores/useMetricsStore';
import { useTrafficStore } from '../stores/useTrafficStore';
import { useVersionStore } from '../stores/useVersionStore';
import { syncDynamicData } from './useEditorCompletion';
import pushService from '../services/pushService';
import { useForceRefreshStore } from '../stores/useForceRefreshStore';
import { usePendingAuthStore } from '../stores/usePendingAuthStore';
import { usePendingIpTlsStore } from '../stores/usePendingIpTlsStore';
import { usePerformanceModeStore } from '../stores/usePerformanceModeStore';

const VERSION_CHECK_INTERVAL = 60 * 60 * 1000;

interface GlobalDataSyncState {
  initialized: boolean;
  versionCheckIntervalId: number | null;
  visibilityPaused: boolean;
  forceRefresh: boolean;
  trafficEnabled: boolean;
}

const globalState: GlobalDataSyncState = {
  initialized: false,
  versionCheckIntervalId: null,
  visibilityPaused: false,
  forceRefresh: false,
  trafficEnabled: false,
};

function shouldAutoOpenVersionModal(): boolean {
  return !navigator.webdriver;
}

export function useGlobalDataSync({ trafficEnabled = true }: { trafficEnabled?: boolean } = {}) {
  const initRef = useRef(false);

  useEffect(() => {
    if (initRef.current || globalState.initialized) {
      return;
    }
    initRef.current = true;
    globalState.initialized = true;

    const proxyStore = useProxyStore.getState();
    const filterPanelStore = useFilterPanelStore.getState();
    const metricsStore = useMetricsStore.getState();
    const versionStore = useVersionStore.getState();
    const performanceModeStore = usePerformanceModeStore.getState();

    const pauseRealtime = () => {
      if (globalState.visibilityPaused) return;
      globalState.visibilityPaused = true;
      if (globalState.trafficEnabled) {
        useTrafficStore.getState().disablePush();
      }
      useMetricsStore.getState().disablePush();
      pushService.disconnect();
    };

    const resumeRealtime = () => {
      if (globalState.forceRefresh) {
        return;
      }
      if (!globalState.visibilityPaused) return;
      globalState.visibilityPaused = false;
      if (globalState.trafficEnabled) {
        const currentTrafficStore = useTrafficStore.getState();
        if (currentTrafficStore.polling && currentTrafficStore.usePush) {
          // Reconnect carries the monotonic last_sequence and the server sends
          // the backlog as initial deltas. A parallel HTTP catch-up duplicates
          // deserialization and merge work exactly when the backlog is largest.
          currentTrafficStore.enablePush();
        }
      }
      useMetricsStore.getState().enablePush({
        needOverview: true,
        needMetrics: true,
      });
    };

    // Only browser-window visibility changes should pause realtime push.
    // In-app tab or route switches must not affect the status bar or traffic subscriptions.
    const onVisibilityChange = () => {
      if (document.visibilityState === 'hidden') {
        pauseRealtime();
      } else {
        resumeRealtime();
      }
    };

    const onPageHide = () => pauseRealtime();
    const onPageShow = () => resumeRealtime();

    const stopAllPolling = () => {
      if (globalState.versionCheckIntervalId) {
        clearInterval(globalState.versionCheckIntervalId);
        globalState.versionCheckIntervalId = null;
      }
      useTrafficStore.getState().stopPolling();
    };

    const onForceRefresh = (data: { reason: string }) => {
      if (globalState.forceRefresh) return;
      globalState.forceRefresh = true;
      stopAllPolling();
      pauseRealtime();
      usePendingAuthStore.getState().stopSSE();
      usePendingIpTlsStore.getState().stopSSE();
      pushService.disableReconnectUntilRefresh();
      useForceRefreshStore.getState().show(data.reason);
    };

    const initializeGlobalData = async () => {
      await versionStore.resumeUpgradeProgress();
      await Promise.allSettled([
        proxyStore.fetchSystemProxy(),
        proxyStore.fetchCliProxy(),
        filterPanelStore.loadFromServer(),
        metricsStore.fetchOverview(),
        versionStore.checkVersion({ skipCache: true }),
        performanceModeStore.fetchPerformanceMode(),
      ]);

      if (globalState.forceRefresh) {
        return;
      }

      metricsStore.enablePush({
        needOverview: true,
        needMetrics: true,
      });

      if (globalState.forceRefresh) {
        return;
      }

      globalState.versionCheckIntervalId = window.setInterval(() => {
        useVersionStore.getState().checkVersion({ skipCache: true });
      }, VERSION_CHECK_INTERVAL);

      syncDynamicData();

      const currentVersionStore = useVersionStore.getState();
      if (shouldAutoOpenVersionModal() && currentVersionStore.hasUpdate) {
        currentVersionStore.setModalVisible(true);
      }
    };

    initializeGlobalData();

    if (import.meta.env.DEV) {
      (window as unknown as Record<string, unknown>).__bifrost_test = {
        pauseRealtime,
        resumeRealtime,
        pushService,
        useTrafficStore,
      };
    }

    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('pagehide', onPageHide);
    window.addEventListener('pageshow', onPageShow);
    const unsubscribeForceRefresh = pushService.onForceRefresh(onForceRefresh);
    const unsubscribeNotification = pushService.onNotification((data) => {
      if (data.notification_type === 'rule_share_imported') {
        message.success(data.message || data.title);
      }
    });

    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange);
      window.removeEventListener('pagehide', onPageHide);
      window.removeEventListener('pageshow', onPageShow);
      unsubscribeForceRefresh();
      unsubscribeNotification();

      stopAllPolling();

      useMetricsStore.getState().disablePush();
      useTrafficStore.getState().stopPolling();
      globalState.initialized = false;
      globalState.visibilityPaused = false;
      globalState.forceRefresh = false;
      globalState.trafficEnabled = false;
    };
  }, []);

  useEffect(() => {
    globalState.trafficEnabled = trafficEnabled;
    const trafficStore = useTrafficStore.getState();
    if (!trafficEnabled || globalState.forceRefresh) {
      trafficStore.disablePush();
      trafficStore.stopPolling();
      return;
    }

    let cancelled = false;
    void trafficStore.fetchInitialData().finally(() => {
      if (cancelled || globalState.forceRefresh || !globalState.trafficEnabled) {
        return;
      }
      if (!useTrafficStore.getState().paused) {
        useTrafficStore.getState().startPolling();
      }
    });

    return () => {
      cancelled = true;
      useTrafficStore.getState().disablePush();
      useTrafficStore.getState().stopPolling();
    };
  }, [trafficEnabled]);
}

export function resetGlobalDataSync() {
  if (globalState.versionCheckIntervalId) {
    clearInterval(globalState.versionCheckIntervalId);
    globalState.versionCheckIntervalId = null;
  }
  globalState.initialized = false;
}

export function isGlobalDataInitialized() {
  return globalState.initialized;
}
