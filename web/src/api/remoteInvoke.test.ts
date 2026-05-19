import { describe, expect, it } from "vitest";
import {
  buildFileAccessPolicy,
  findFileAccessPolicyForGrant,
  getFileAccessPolicyAccess,
  getFileAccessPolicyRootScope,
  getCallArgsPreviewSource,
  normalizeRemoteInvokeSshCallerInfo,
  normalizeRemoteInvokeSshKeyRecord,
  type FileAccessConfig,
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

describe("file access policy helpers", () => {
  const grant = {
    grant_id: "grant-active-1",
    caller_display_name: "mira",
    caller_fingerprint: "abc123",
    ssh_key_fingerprint: null,
  };

  it("builds a read-only policy for selected roots from an active grant", () => {
    const policy = buildFileAccessPolicy(
      grant,
      "read",
      "selected",
      ["/home/user/projects/bifrost"],
    );

    expect(policy.grant_id).toBe("grant-active-1");
    expect(policy.name).toBe("mira");
    expect(policy.roots).toEqual(["/home/user/projects/bifrost"]);
    expect(getFileAccessPolicyAccess(policy)).toBe("read");
    expect(getFileAccessPolicyRootScope(policy)).toBe("selected");
    expect(policy.ops).toEqual(["read", "list", "stat", "glob", "search", "hash"]);
  });

  it("builds a read-write all-directories policy as root slash", () => {
    const policy = buildFileAccessPolicy(grant, "read_write", "all", []);

    expect(policy.roots).toEqual(["/"]);
    expect(getFileAccessPolicyAccess(policy)).toBe("read_write");
    expect(getFileAccessPolicyRootScope(policy)).toBe("all");
    expect(policy.ops).toContain("apply_patch");
    expect(policy.allow_recursive_delete).toBe(false);
  });

  it("resolves the effective policy with grant id before ssh and caller fingerprints", () => {
    const config: FileAccessConfig = {
      grant: [
        {
          match: { caller_fingerprint: "abc123" },
          roots: ["/caller"],
          ops: ["read"],
        },
        {
          match: { ssh_fingerprint: "ssh-fp-1" },
          roots: ["/"],
          ops: ["read", "list", "stat", "glob", "search", "hash", "write"],
        },
        {
          grant_id: "grant-active-1",
          roots: ["/exact"],
          ops: ["read", "list"],
        },
      ],
    };

    expect(findFileAccessPolicyForGrant(config, {
      ...grant,
      ssh_key_fingerprint: "ssh-fp-1",
    })?.roots).toEqual(["/exact"]);
  });

  it("falls back to ssh-key policy so grants inherit all-directories defaults", () => {
    const config: FileAccessConfig = {
      grant: [
        {
          match: { ssh_fingerprint: "ssh-fp-1" },
          roots: ["/"],
          ops: ["read", "list", "stat", "glob", "search", "hash", "write"],
        },
      ],
    };

    const policy = findFileAccessPolicyForGrant(config, {
      ...grant,
      grant_id: "new-ssh-grant",
      ssh_key_fingerprint: "ssh-fp-1",
    });

    expect(policy?.roots).toEqual(["/"]);
    expect(getFileAccessPolicyRootScope(policy!)).toBe("all");
    expect(getFileAccessPolicyAccess(policy!)).toBe("read_write");
  });
});
