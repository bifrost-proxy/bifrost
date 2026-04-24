import { describe, expect, it } from "vitest";
import {
  getCallArgsPreviewSource,
  normalizeRemoteInvokeSshCallerInfo,
  normalizeRemoteInvokeSshKeyRecord,
} from "./remoteInvoke";

describe("remoteInvoke SSH key normalization", () => {
  it("parses last caller info from persisted json text", () => {
    const caller = normalizeRemoteInvokeSshCallerInfo(
      JSON.stringify({
        hostname: "ci-runner",
        username: "bifrost",
        platform: "linux",
        source_ip: "10.0.0.8",
      }),
    );

    expect(caller).toEqual({
      hostname: "ci-runner",
      username: "bifrost",
      platform: "linux",
      user_agent: undefined,
      source_ip: "10.0.0.8",
      ip: undefined,
    });
  });

  it("accepts fallback fingerprint and caller_info fields from progressive backend payloads", () => {
    const record = normalizeRemoteInvokeSshKeyRecord({
      device_code: "BF-A1B2C3D4E5F6A7B8",
      label: "CI Agent",
      fingerprint: "SHA256:abc123",
      grant_mode: "1h",
      last_caller_info: {
        hostname: "macbook-pro",
        sourceIp: "192.168.0.10",
        platform: "macOS",
      },
    });

    expect(record).toEqual({
      id: "BF-A1B2C3D4E5F6A7B8",
      label: "CI Agent",
      device_code: "BF-A1B2C3D4E5F6A7B8",
      ssh_key_fingerprint: "SHA256:abc123",
      status: "active",
      grant_mode: "1h",
      created_at: undefined,
      last_used_at: undefined,
      last_caller_info: {
        hostname: "macbook-pro",
        username: undefined,
        platform: "macOS",
        user_agent: undefined,
        source_ip: "192.168.0.10",
        ip: undefined,
      },
    });
  });

  it("returns null when the backend has not provisioned a key yet", () => {
    expect(normalizeRemoteInvokeSshKeyRecord(null)).toBeNull();
    expect(normalizeRemoteInvokeSshKeyRecord({})).toBeNull();
  });
});

describe("getCallArgsPreviewSource", () => {
  it("prefers masked args from command_summary", () => {
    expect(
      getCallArgsPreviewSource({
        command_summary: {
          command_preview: "search.get",
          masked_args_json: '{"query":"***"}',
        },
        command: {
          command: "search.get",
          args_json: '{"query":"needle"}',
        },
      }),
    ).toBe('{"query":"***"}');
  });

  it("falls back to decrypted command args when masked summary is missing", () => {
    expect(
      getCallArgsPreviewSource({
        command_summary: {
          command_preview: "search.get",
        },
        command: {
          command: "search.get",
          args_json: '{"query":"needle","max_results":5}',
        },
      }),
    ).toBe('{"query":"needle","max_results":5}');
  });

  it("falls back to typed query args when args_json is absent", () => {
    expect(
      getCallArgsPreviewSource({
        command_summary: {
          command_preview: "search.stream",
        },
        command: {
          command: "",
          query: {
            type: "search",
            args: {
              keyword: "needle",
              limit: 5,
              max_scan: 50,
            },
          },
        },
      }),
    ).toBe('{"keyword":"needle","limit":5,"max_scan":50}');
  });
});
