import { describe, expect, it } from "vitest";
import type { SyncProviderStatus } from "../../../api/sync";
import {
  shouldShowSyncProviderOverviewAlert,
  syncProviderActionableError,
  syncProviderStatusBadge,
} from "./SyncTab";

function provider(
  overrides: Partial<SyncProviderStatus> = {},
): SyncProviderStatus {
  return {
    id: "github_gist",
    name: "GitHub Gist",
    description: "Public GitHub Gist-backed portable sync provider.",
    remote_base_url: "https://api.github.com/gists",
    connected: true,
    enabled: true,
    reachable: true,
    authorized: true,
    reason: "ready",
    last_error: null,
    last_sync_at: null,
    last_sync_action: null,
    user: null,
    capabilities: {
      rules_sync: true,
      config_sync: true,
      remote_invoke: false,
    },
    remote_invoke_registered: false,
    ...overrides,
  };
}

describe("syncProviderStatusBadge", () => {
  it("asks users to reconnect when a saved provider session has an auth error", () => {
    expect(
      syncProviderStatusBadge(
        provider({
          authorized: false,
          reason: "error",
          last_error: "GitHub token is invalid or missing the gist scope",
        }),
      ),
    ).toEqual({ color: "red", label: "Reconnect required" });
  });

  it("keeps healthy saved provider sessions connected", () => {
    expect(syncProviderStatusBadge(provider())).toEqual({
      color: "green",
      label: "Connected",
    });
  });

  it("ignores a stale persisted error after the provider is authorized and ready", () => {
    const healthy = provider({ last_error: "stale login error" });
    expect(syncProviderStatusBadge(healthy)).toEqual({
      color: "green",
      label: "Connected",
    });
    expect(syncProviderActionableError(healthy)).toBeNull();
  });

  it("distinguishes an authorized sync failure from an authentication failure", () => {
    const failed = provider({
      reason: "error",
      last_error: "failed to create remote rule 'bad name'",
    });
    expect(syncProviderStatusBadge(failed)).toEqual({
      color: "red",
      label: "Sync error",
    });
    expect(syncProviderActionableError(failed)).toContain("bad name");
  });
});

describe("shouldShowSyncProviderOverviewAlert", () => {
  it("hides the global pluggable-sync hint when any provider is connected", () => {
    expect(
      shouldShowSyncProviderOverviewAlert([
        provider({ id: "bytedance_internal", name: "ByteDance Internal" }),
        provider({
          id: "bifrost_cloud",
          name: "Bifrost Cloud",
          connected: false,
          authorized: false,
          reachable: false,
          reason: "unreachable",
        }),
      ]),
    ).toBe(false);
  });

  it("shows the global pluggable-sync hint only when no provider is connected", () => {
    expect(
      shouldShowSyncProviderOverviewAlert([
        provider({
          connected: false,
          authorized: false,
          reason: "unauthorized",
        }),
        provider({
          id: "bifrost_cloud",
          name: "Bifrost Cloud",
          connected: false,
          authorized: false,
          reachable: false,
          reason: "unreachable",
        }),
      ]),
    ).toBe(true);
  });
});
