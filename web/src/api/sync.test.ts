import { describe, expect, it } from "vitest";
import { getDefaultRemoteBaseUrl } from "./sync";

describe("sync default remote URL", () => {
  it("decodes to an HTTPS URL when consumed", () => {
    const remote = getDefaultRemoteBaseUrl();

    expect(new URL(remote).protocol).toBe("https:");
    expect(new URL(remote).hostname).not.toBe("");
  });
});
