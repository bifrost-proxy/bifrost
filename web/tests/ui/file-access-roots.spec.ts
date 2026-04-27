import { test, expect } from "@playwright/test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { apiBase } from "./helpers/admin-helpers";

/**
 * HTTP-level regression for the new File Access per-grant roots picker API:
 *   POST   /remote-invoke/file-access/validate-path
 *   GET    /remote-invoke/file-access/grants/<id>/roots
 *   POST   /remote-invoke/file-access/grants/<id>/roots
 *   DELETE /remote-invoke/file-access/grants/<id>/roots
 *
 * These tests do not depend on a rendered UI component (the frontend picker
 * lands in a later phase). They drive the daemon backend directly through
 * Playwright's APIRequestContext so regressions surface as soon as the
 * handler contract drifts.
 */

const TEST_GRANT_ID = "ui-e2e-file-access-grant";

async function seedGrant(dataDir: string) {
  const cfg = path.join(dataDir, "file-access.toml");
  let current = "";
  try {
    current = await fs.readFile(cfg, "utf8");
  } catch {
    current = "";
  }
  // Don't double-seed if a prior test already appended this grant.
  if (!current.includes(`grant_id = "${TEST_GRANT_ID}"`)) {
    const block = `\n[[grant]]\nmatch.grant_id = "${TEST_GRANT_ID}"\nroots = []\nops = ["read"]\n`;
    await fs.writeFile(cfg, current + block, "utf8");
  }
}

async function removeSeedGrant(dataDir: string) {
  const cfg = path.join(dataDir, "file-access.toml");
  try {
    const current = await fs.readFile(cfg, "utf8");
    // Strip the [[grant]] block we added. Naive but sufficient for an
    // isolated test dataDir.
    const cleaned = current.replace(
      new RegExp(
        `\\n\\[\\[grant\\]\\]\\s*\\nmatch\\.grant_id = "${TEST_GRANT_ID}"[\\s\\S]*?(?=\\n\\[|$)`,
        "m",
      ),
      "",
    );
    await fs.writeFile(cfg, cleaned, "utf8");
  } catch {
    // nothing to clean
  }
}

test.describe("File Access per-grant roots API", () => {
  let dataDir: string;
  let tmpDir: string;

  test.beforeAll(async () => {
    dataDir = process.env.BIFROST_DATA_DIR as string;
    expect(dataDir, "BIFROST_DATA_DIR must be set by global-setup").toBeTruthy();
    await seedGrant(dataDir);
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "bifrost-ui-roots-"));
  });

  test.afterAll(async () => {
    await removeSeedGrant(dataDir);
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  test("validate-path reports is_dir for real directories", async ({ request }) => {
    const res = await request.post(`${apiBase}/remote-invoke/file-access/validate-path`, {
      data: { path: tmpDir },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.exists).toBe(true);
    expect(body.is_dir).toBe(true);
    expect(typeof body.canonical).toBe("string");
  });

  test("validate-path rejects non-existent paths with a reason", async ({ request }) => {
    const res = await request.post(`${apiBase}/remote-invoke/file-access/validate-path`, {
      data: { path: "/definitely/not/here/xyz-bifrost-ui" },
    });
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.exists).toBe(false);
    expect(body.is_dir).toBe(false);
    expect(typeof body.reason).toBe("string");
  });

  test("validate-path rejects non-POST methods", async ({ request }) => {
    const res = await request.get(`${apiBase}/remote-invoke/file-access/validate-path`);
    expect(res.status()).toBe(405);
  });

  test("GET roots returns empty list for a freshly seeded grant", async ({ request }) => {
    const res = await request.get(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
    );
    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
    expect(body.grant_id).toBe(TEST_GRANT_ID);
    expect(Array.isArray(body.roots)).toBe(true);
    expect(body.roots).toEqual([]);
  });

  test("POST add root then GET reflects the new root, idempotent on repeat", async ({
    request,
  }) => {
    const subdir = path.join(tmpDir, "repo");
    await fs.mkdir(subdir, { recursive: true });

    const addRes = await request.post(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      { data: { path: subdir } },
    );
    expect(addRes.status()).toBe(200);
    const addBody = await addRes.json();
    expect(addBody.success).toBe(true);
    expect(addBody.data.already_present).toBe(false);
    expect(typeof addBody.data.canonical).toBe("string");

    const listRes = await request.get(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
    );
    const listBody = await listRes.json();
    expect(listBody.roots).toContain(addBody.data.canonical);

    // Repeat add => already_present=true, no duplicate.
    const repeatRes = await request.post(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      { data: { path: subdir } },
    );
    expect(repeatRes.status()).toBe(200);
    const repeatBody = await repeatRes.json();
    expect(repeatBody.data.already_present).toBe(true);

    const listRes2 = await request.get(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
    );
    const listBody2 = await listRes2.json();
    const occurrences = listBody2.roots.filter((r: string) => r === addBody.data.canonical).length;
    expect(occurrences).toBe(1);

    // Cleanup via DELETE.
    const delRes = await request.delete(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      { data: { path: addBody.data.canonical } },
    );
    expect(delRes.status()).toBe(200);
    const delBody = await delRes.json();
    expect(delBody.removed).toBe(true);
  });

  test("POST with a nonexistent path returns 400 InvalidPath", async ({ request }) => {
    const res = await request.post(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      { data: { path: "/does/not/exist/bifrost-ui-e2e" } },
    );
    expect(res.status()).toBe(400);
    const body = await res.json();
    expect(body.error).toContain("path does not exist");
  });

  test("POST on an unknown grant returns 404 GrantNotFound", async ({ request }) => {
    const res = await request.post(
      `${apiBase}/remote-invoke/file-access/grants/no-such-grant-xyz/roots`,
      { data: { path: tmpDir } },
    );
    expect(res.status()).toBe(404);
    const body = await res.json();
    expect(body.error).toContain("not found");
  });

  test("PUT on roots route returns 405", async ({ request }) => {
    const res = await request.fetch(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      { method: "PUT", data: {} },
    );
    expect(res.status()).toBe(405);
  });

  test("Malformed JSON body returns 400 with a descriptive error", async ({ request }) => {
    const res = await request.post(
      `${apiBase}/remote-invoke/file-access/grants/${TEST_GRANT_ID}/roots`,
      {
        headers: { "Content-Type": "application/json" },
        data: "not json" as unknown as object,
      },
    );
    expect(res.status()).toBe(400);
    const body = await res.json();
    expect(body.error.toLowerCase()).toContain("invalid");
  });
});
