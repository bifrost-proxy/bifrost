// @vitest-environment node
import { afterEach, describe, expect, it } from "vitest";
import {
  startMockSyncServer,
  type MockSyncEnv,
  type MockSyncServer,
} from "../ui/helpers/admin-helpers";

const servers: MockSyncServer[] = [];
const seed: MockSyncEnv = {
  id: "availability-rule",
  user_id: "ui-sync-user",
  name: "availability-rule",
  rule: "example.test status://200",
  create_time: "2026-01-01T00:00:00Z",
  update_time: "2026-01-01T00:00:00Z",
};

afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.close()));
});

async function startServer() {
  const server = await startMockSyncServer([seed]);
  servers.push(server);
  return server;
}

describe("mock Sync server availability", () => {
  it("returns 503 for probes, login, reads and writes without mutating rules", async () => {
    const server = await startServer();
    server.setAvailable(false);

    for (const [method, endpoint] of [
      ["GET", "/v4/sso/check"],
      ["GET", "/v4/sso/info"],
      ["POST", "/v4/sso/logout"],
      ["GET", "/v4/sso/login?next=http%3A%2F%2F127.0.0.1%2Flogin.html"],
      ["GET", "/v4/env"],
      ["POST", "/v4/env"],
      ["PATCH", `/v4/env/${seed.id}`],
      ["DELETE", `/v4/env/${seed.id}`],
    ]) {
      const response = await fetch(`${server.baseUrl}${endpoint}`, {
        method,
        headers: { "X-Bifrost-Token": server.token },
        body: ["POST", "PATCH"].includes(method)
          ? JSON.stringify({ ...seed, rule: "should not be saved" })
          : undefined,
        redirect: "manual",
      });
      expect(response.status, `${method} ${endpoint}`).toBe(503);
      expect(await response.json()).toEqual({
        code: -1,
        message: "mock sync server unavailable",
      });
    }
    expect(server.listEnvs()).toEqual([seed]);
  });

  it("recovers repeatedly at the same URL with the original token and stored rules", async () => {
    const server = await startServer();
    const originalUrl = server.baseUrl;
    const originalToken = server.token;
    const headers = { "X-Bifrost-Token": originalToken };

    const initialProbe = await fetch(`${originalUrl}/v4/sso/check`, {
      headers,
    });
    expect(initialProbe.status).toBe(200);
    expect(await initialProbe.json()).toMatchObject({
      data: { token: originalToken, user_id: server.user.user_id },
    });

    let expectedRule = seed.rule;
    for (let cycle = 0; cycle < 2; cycle += 1) {
      server.setAvailable(false);
      // Reachability probes do not carry the saved authentication token.
      const offlineProbe = await fetch(`${originalUrl}/v4/sso/check`);
      expect(offlineProbe.status).toBe(503);
      await offlineProbe.json();

      server.setAvailable(true);
      expect(server.baseUrl).toBe(originalUrl);
      expect(server.token).toBe(originalToken);
      const userResponse = await fetch(`${originalUrl}/v4/sso/info`, {
        headers,
      });
      expect(userResponse.status).toBe(200);
      expect(await userResponse.json()).toMatchObject({ data: server.user });
      const anonymousProbe = await fetch(`${originalUrl}/v4/sso/check`);
      expect(anonymousProbe.status).toBe(401);
      await anonymousProbe.json();
      const listResponse = await fetch(`${originalUrl}/v4/env`, { headers });
      expect(listResponse.status).toBe(200);
      expect(await listResponse.json()).toMatchObject({
        data: {
          list: [
            { ...seed, rule: expectedRule, update_time: expect.any(String) },
          ],
        },
      });

      expectedRule = `example.test status://${201 + cycle}`;
      const updateResponse = await fetch(`${originalUrl}/v4/env/${seed.id}`, {
        method: "PATCH",
        headers,
        body: JSON.stringify({ rule: expectedRule }),
      });
      expect(updateResponse.status).toBe(200);
      expect(await updateResponse.json()).toMatchObject({
        data: { id: seed.id, rule: expectedRule },
      });
      expect(server.listEnvs()).toEqual([
        { ...seed, rule: expectedRule, update_time: expect.any(String) },
      ]);
    }
  });
});
