import { describe, expect, it } from "vitest";
import type { UserPassStatus } from "../../../types";
import {
  createUserPassDraft,
  mergeUserPassRuntimeState,
  userPassConfigSignature,
} from "./AccessControlTab";

const userpass = (overrides: Partial<UserPassStatus> = {}): UserPassStatus => ({
  enabled: true,
  loopback_requires_auth: true,
  accounts: [
    {
      username: "cli-user",
      enabled: true,
      has_password: true,
      last_connected_at: "2026-07-10T05:00:00Z",
    },
  ],
  ...overrides,
});

describe("createUserPassDraft", () => {
  it("initializes a newly mounted access tab from an already loaded status", () => {
    expect(createUserPassDraft(userpass())).toEqual({
      enabled: true,
      loopbackRequiresAuth: true,
      accounts: [
        {
          key: "cli-user",
          username: "cli-user",
          password: "",
          enabled: true,
          hasPassword: true,
          lastConnectedAt: "2026-07-10T05:00:00Z",
        },
      ],
    });
  });

  it("uses a safe empty draft before status loading finishes", () => {
    expect(createUserPassDraft(undefined)).toEqual({
      enabled: false,
      loopbackRequiresAuth: false,
      accounts: [],
    });
  });
});

describe("userPassConfigSignature", () => {
  it("ignores runtime connection timestamps so they do not overwrite unsaved drafts", () => {
    expect(
      userPassConfigSignature(
        userpass({
          accounts: [
            {
              username: "cli-user",
              enabled: true,
              has_password: true,
              last_connected_at: "2026-07-10T06:00:00Z",
            },
          ],
        }),
      ),
    ).toBe(userPassConfigSignature(userpass()));
  });

  it("detects changes to global, loopback, account, and password-marker config", () => {
    const original = userPassConfigSignature(userpass());

    expect(userPassConfigSignature(userpass({ enabled: false }))).not.toBe(
      original,
    );
    expect(
      userPassConfigSignature(userpass({ loopback_requires_auth: false })),
    ).not.toBe(original);
    expect(
      userPassConfigSignature(
        userpass({
          accounts: [
            {
              username: "cli-user",
              enabled: false,
              has_password: true,
              last_connected_at: null,
            },
          ],
        }),
      ),
    ).not.toBe(original);
  });
});

describe("mergeUserPassRuntimeState", () => {
  it("updates connection timestamps without overwriting an unsaved account draft", () => {
    const draft = createUserPassDraft(userpass());
    draft.accounts[0] = {
      ...draft.accounts[0],
      username: "unsaved-name",
      password: "unsaved-password",
      enabled: false,
    };
    const server = userpass({
      accounts: [
        {
          username: "unsaved-name",
          enabled: true,
          has_password: true,
          last_connected_at: "2026-07-10T07:00:00Z",
        },
      ],
    });

    expect(mergeUserPassRuntimeState(draft.accounts, server)).toEqual([
      {
        ...draft.accounts[0],
        lastConnectedAt: "2026-07-10T07:00:00Z",
      },
    ]);
  });

  it("keeps the same array when runtime state has not changed", () => {
    const accounts = createUserPassDraft(userpass()).accounts;
    expect(mergeUserPassRuntimeState(accounts, userpass())).toBe(accounts);
  });
});
