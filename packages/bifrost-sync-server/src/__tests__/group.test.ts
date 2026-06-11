import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import http from 'http';
import fs from 'fs';
import path from 'path';
import { createSyncServer, type SyncServerInstance, type SyncServerConfig } from '../index';

const TEST_DATA_DIR = path.join(__dirname, '.test-data-group');
const TEST_PORT = 0;

let server: SyncServerInstance;
let baseUrl: string;

function req(
  method: string,
  urlPath: string,
  body?: unknown,
  token?: string,
): Promise<{ status: number; data: { code: number; message: string; data?: unknown } }> {
  return new Promise((resolve, reject) => {
    const url = new URL(urlPath, baseUrl);
    const options: http.RequestOptions = {
      method,
      hostname: url.hostname,
      port: url.port,
      path: url.pathname + url.search,
      headers: { 'Content-Type': 'application/json' },
    };
    if (token) {
      (options.headers as Record<string, string>)['x-bifrost-token'] = token;
    }
    const r = http.request(options, (res) => {
      let chunks = '';
      res.on('data', (c) => (chunks += c));
      res.on('end', () => {
        try {
          resolve({ status: res.statusCode!, data: JSON.parse(chunks) });
        } catch {
          resolve({ status: res.statusCode!, data: { code: -1, message: chunks } });
        }
      });
    });
    r.on('error', reject);
    if (body !== undefined) {
      r.write(JSON.stringify(body));
    }
    r.end();
  });
}

async function registerUser(userId: string, password: string): Promise<string> {
  const res = await req('POST', '/v4/sso/register', { user_id: userId, password });
  expect(res.data.code, JSON.stringify(res.data)).toBe(0);
  return (res.data.data as { token: string }).token;
}

beforeAll(async () => {
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
  fs.mkdirSync(TEST_DATA_DIR, { recursive: true });

  const config: SyncServerConfig = {
    server: {
      port: TEST_PORT,
      host: '127.0.0.1',
      rate_limit_per_ip: 1000,
      auth_rate_limit_per_ip: 1000,
    },
    storage: { type: 'sqlite', sqlite: { data_dir: TEST_DATA_DIR } },
    auth: { mode: 'password' },
  };

  server = createSyncServer(config);
  await new Promise<void>((resolve) => {
    server.server.listen(0, '127.0.0.1', () => {
      const addr = server.server.address();
      if (addr && typeof addr === 'object') {
        server.port = addr.port;
      }
      resolve();
    });
  });
  baseUrl = `http://127.0.0.1:${server.port}`;
});

afterAll(async () => {
  await server.close();
  if (fs.existsSync(TEST_DATA_DIR)) {
    fs.rmSync(TEST_DATA_DIR, { recursive: true });
  }
});

describe('Group API - Authentication', () => {
  it('should reject unauthenticated requests', async () => {
    const res = await req('GET', '/v4/group');
    expect(res.status).toBe(401);
    expect(res.data.code).toBe(-10001);
  });

  it('should reject requests with invalid token', async () => {
    const res = await req('GET', '/v4/group', undefined, 'invalid-token');
    expect(res.status).toBe(401);
  });
});

describe('Group API - Full Lifecycle', () => {
  let ownerToken: string;
  let memberToken: string;
  let outsiderToken: string;
  let groupId: string;

  beforeAll(async () => {
    ownerToken = await registerUser('group_owner', 'password123');
    memberToken = await registerUser('group_member', 'password123');
    outsiderToken = await registerUser('group_outsider', 'password123');
  });

  describe('Create Group', () => {
    it('should create a private group', async () => {
      const res = await req('POST', '/v4/group', {
        name: 'Test Private Group',
        description: 'A test private group',
        visibility: 'private',
      }, ownerToken);

      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const group = res.data.data as { id: string; name: string; visibility: number; level: number };
      expect(group.name).toBe('Test Private Group');
      expect(group.visibility).toBe(0);
      expect(group.level).toBe(2);
      groupId = group.id;
    });

    it('should fail to create group without name', async () => {
      const res = await req('POST', '/v4/group', {
        description: 'no name group',
      }, ownerToken);
      expect(res.status).toBe(400);
    });

    it('should create a public group', async () => {
      const res = await req('POST', '/v4/group', {
        name: 'Public Group',
        description: 'A public group',
        visibility: 'public',
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const group = res.data.data as { visibility: number };
      expect(group.visibility).toBe(1);
    });

    it('should default to private visibility', async () => {
      const res = await req('POST', '/v4/group', {
        name: 'Default Visibility Group',
      }, ownerToken);
      expect(res.status).toBe(200);
      const group = res.data.data as { visibility: number };
      expect(group.visibility).toBe(0);
    });
  });

  describe('Read Group', () => {
    it('should read group as owner', async () => {
      const res = await req('GET', `/v4/group/${groupId}`, undefined, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const group = res.data.data as { id: string; name: string; level: number };
      expect(group.id).toBe(groupId);
      expect(group.level).toBe(2);
    });

    it('should deny access to private group for outsider', async () => {
      const res = await req('GET', `/v4/group/${groupId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should return 404 for non-existent group', async () => {
      const res = await req('GET', '/v4/group/nonexistent-id', undefined, ownerToken);
      expect(res.status).toBe(404);
    });
  });

  describe('Search Groups', () => {
    it('should list groups the user belongs to (no keyword)', async () => {
      const res = await req('GET', '/v4/group', undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[]; total: number };
      expect(data.list.length).toBeGreaterThanOrEqual(1);
      expect(data.total).toBeGreaterThanOrEqual(1);
    });

    it('should return empty list for user with no groups', async () => {
      const res = await req('GET', '/v4/group', undefined, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[]; total: number };
      expect(data.list.length).toBe(0);
    });

    it('should search groups by keyword', async () => {
      const res = await req('GET', '/v4/group?keyword=Test', undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { name: string }[] };
      expect(data.list.some((g) => g.name.includes('Test'))).toBe(true);
    });

    it('should search groups by keyword with pagination', async () => {
      const res = await req('GET', '/v4/group?keyword=Group&offset=0&limit=1', undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[]; total: number };
      expect(data.list.length).toBeLessThanOrEqual(1);
    });
  });

  describe('Update Group', () => {
    it('should update group as owner', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}`, {
        name: 'Updated Group Name',
        description: 'Updated description',
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const group = res.data.data as { name: string; what: string };
      expect(group.name).toBe('Updated Group Name');
      expect(group.what).toBe('Updated description');
    });

    it('should deny update for outsider', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}`, {
        name: 'Hacker Name',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Invite Members', () => {
    it('should invite a member as owner', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: groupId,
        user_id: ['group_member'],
        level: 0,
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('should not duplicate invite existing member', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: groupId,
        user_id: ['group_member'],
      }, ownerToken);
      expect(res.status).toBe(200);
    });

    it('should deny invite by outsider', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: groupId,
        user_id: ['group_outsider'],
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should deny invite by regular member (level 0)', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: groupId,
        user_id: ['group_outsider'],
      }, memberToken);
      expect(res.status).toBe(403);
    });

    it('should fail with missing user_id', async () => {
      const res = await req('POST', `/v4/group/invite`, { group_id: groupId }, ownerToken);
      expect(res.status).toBe(400);
    });
  });

  describe('List Members', () => {
    it('should list members as owner', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { user_id: string; level: number }[]; total: number };
      expect(data.total).toBe(2);
      const owner = data.list.find((m) => m.user_id === 'group_owner');
      expect(owner).toBeDefined();
      expect(owner!.level).toBe(2);
      const member = data.list.find((m) => m.user_id === 'group_member');
      expect(member).toBeDefined();
      expect(member!.level).toBe(0);
    });

    it('should list members as member', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members`, undefined, memberToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[]; total: number };
      expect(data.total).toBe(2);
    });

    it('should deny listing members for outsider', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should filter members by keyword', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members?keyword=owner`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { user_id: string }[]; total: number };
      expect(data.list.length).toBe(1);
      expect(data.list[0].user_id).toBe('group_owner');
    });

    it('should paginate members', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members?offset=0&limit=1`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[]; total: number };
      expect(data.list.length).toBe(1);
      expect(data.total).toBe(2);
    });
  });

  describe('Update Member Level', () => {
    it('should promote member to master', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/member/group_member`, {
        level: 1,
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('should verify member is now master', async () => {
      const res = await req('GET', `/v4/group/${groupId}/members`, undefined, ownerToken);
      const data = res.data.data as { list: { user_id: string; level: number }[] };
      const member = data.list.find((m) => m.user_id === 'group_member');
      expect(member!.level).toBe(1);
    });

    it('should deny changing own level', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/member/group_owner`, {
        level: 0,
      }, ownerToken);
      expect(res.status).toBe(400);
    });

    it('should deny level change by outsider', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/member/group_member`, {
        level: 0,
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should fail without level field', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/member/group_member`, {}, ownerToken);
      expect(res.status).toBe(400);
    });
  });

  describe('Master Permissions', () => {
    it('master should be able to update group', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}`, {
        description: 'Master updated description',
      }, memberToken);
      expect(res.status).toBe(200);
      const group = res.data.data as { what: string };
      expect(group.what).toBe('Master updated description');
    });

    it('master should be able to invite members', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: groupId,
        user_id: ['group_outsider'],
        level: 0,
      }, memberToken);
      expect(res.status).toBe(200);
    });

    it('master should not be able to delete group', async () => {
      const res = await req('DELETE', `/v4/group/${groupId}`, undefined, memberToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Remove Member', () => {
    it('should remove a member as owner', async () => {
      const res = await req('DELETE', `/v4/group/${groupId}/member/group_outsider`, undefined, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('should deny removing self', async () => {
      const res = await req('DELETE', `/v4/group/${groupId}/member/group_owner`, undefined, ownerToken);
      expect(res.status).toBe(400);
    });

    it('should deny remove by outsider', async () => {
      const res = await req('DELETE', `/v4/group/${groupId}/member/group_member`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Group Settings', () => {
    it('should get group settings as owner', async () => {
      const res = await req('GET', `/v4/group/${groupId}/setting`, undefined, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const setting = res.data.data as { status: boolean; level: number };
      expect(setting.status).toBe(true);
      expect(setting.level).toBe(0);
    });

    it('should get group settings as master', async () => {
      const res = await req('GET', `/v4/group/${groupId}/setting`, undefined, memberToken);
      expect(res.status).toBe(200);
    });

    it('should deny settings access for outsider', async () => {
      const res = await req('GET', `/v4/group/${groupId}/setting`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should update group settings', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/setting`, {
        status: false,
        level: 1,
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('should verify updated settings', async () => {
      const res = await req('GET', `/v4/group/${groupId}/setting`, undefined, ownerToken);
      const setting = res.data.data as { status: boolean; level: number };
      expect(setting.status).toBe(false);
      expect(setting.level).toBe(1);
    });

    it('should revert group settings to private', async () => {
      const res = await req('PATCH', `/v4/group/${groupId}/setting`, {
        status: true,
        level: 0,
      }, ownerToken);
      expect(res.status).toBe(200);
    });
  });

  describe('Public Group Access', () => {
    let publicGroupId: string;

    beforeAll(async () => {
      const res = await req('POST', '/v4/group', {
        name: 'Public Access Test',
        visibility: 'public',
      }, ownerToken);
      publicGroupId = (res.data.data as { id: string }).id;
    });

    it('outsider should read public group', async () => {
      const res = await req('GET', `/v4/group/${publicGroupId}`, undefined, outsiderToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('outsider should NOT list members of public group', async () => {
      const res = await req('GET', `/v4/group/${publicGroupId}/members`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT update public group', async () => {
      const res = await req('PATCH', `/v4/group/${publicGroupId}`, {
        name: 'Hacked',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT delete public group', async () => {
      const res = await req('DELETE', `/v4/group/${publicGroupId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT modify public group settings', async () => {
      const res = await req('PATCH', `/v4/group/${publicGroupId}/setting`, {
        status: false,
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT invite members to public group', async () => {
      const res = await req('POST', `/v4/group/invite`, {
        group_id: publicGroupId,
        user_id: ['group_outsider'],
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('public group should default to status=true', async () => {
      const res = await req('GET', `/v4/group/${publicGroupId}/setting`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const setting = res.data.data as { status: boolean };
      expect(setting.status).toBe(true);
    });

    it('public group search should be visible to outsider', async () => {
      const res = await req('GET', '/v4/group?keyword=Public+Access', undefined, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { id: string }[] };
      expect(data.list.some((g) => g.id === publicGroupId)).toBe(true);
    });

    it('private group should NOT be visible in search to outsider', async () => {
      const res = await req('GET', `/v4/group?keyword=Updated+Group`, undefined, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { id: string }[] };
      expect(data.list.some((g) => g.id === groupId)).toBe(false);
    });
  });

  describe('Leave Group', () => {
    let leaveGroupId: string;
    let user2Token: string;

    beforeAll(async () => {
      user2Token = await registerUser('leave_user', 'password123');
      const res = await req('POST', '/v4/group', {
        name: 'Leave Test Group',
      }, ownerToken);
      leaveGroupId = (res.data.data as { id: string }).id;

      await req('POST', `/v4/group/invite`, {
        group_id: leaveGroupId,
        user_id: ['leave_user'],
      }, ownerToken);
    });

    it('member should be able to leave group', async () => {
      const res = await req('POST', `/v4/group/${leaveGroupId}/leave`, undefined, user2Token);
      expect(res.status).toBe(200);
    });

    it('sole owner should NOT be able to leave', async () => {
      const res = await req('POST', `/v4/group/${leaveGroupId}/leave`, undefined, ownerToken);
      expect(res.status).toBe(400);
      expect(res.data.message).toContain('only owner');
    });

    it('outsider should NOT be able to leave a group they are not in', async () => {
      const res = await req('POST', `/v4/group/${leaveGroupId}/leave`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('owner can leave if there is another owner', async () => {
      await req('POST', `/v4/group/invite`, {
        group_id: leaveGroupId,
        user_id: ['leave_user'],
        level: 2,
      }, ownerToken);

      const res = await req('POST', `/v4/group/${leaveGroupId}/leave`, undefined, ownerToken);
      expect(res.status).toBe(200);
    });
  });

  describe('Delete Group', () => {
    let deleteGroupId: string;

    beforeAll(async () => {
      const res = await req('POST', '/v4/group', {
        name: 'Delete Test Group',
      }, ownerToken);
      deleteGroupId = (res.data.data as { id: string }).id;
    });

    it('should deny delete by outsider', async () => {
      const res = await req('DELETE', `/v4/group/${deleteGroupId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('should delete group as owner', async () => {
      const res = await req('DELETE', `/v4/group/${deleteGroupId}`, undefined, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('should return 404 after deletion', async () => {
      const res = await req('GET', `/v4/group/${deleteGroupId}`, undefined, ownerToken);
      expect(res.status).toBe(404);
    });

    it('members should be cleaned up after deletion', async () => {
      const res = await req('GET', `/v4/group/${deleteGroupId}/members`, undefined, ownerToken);
      expect(res.status).toBe(403);
    });
  });
});

describe('Group API - Edge Cases', () => {
  let token: string;

  beforeAll(async () => {
    token = await registerUser('edge_user', 'password123');
  });

  it('should return false for unmatched routes', async () => {
    const res = await req('GET', '/v4/unknown', undefined, token);
    expect(res.status).toBe(404);
  });

  it('should handle multiple groups correctly', async () => {
    const groups: string[] = [];
    for (let i = 0; i < 5; i++) {
      const res = await req('POST', '/v4/group', {
        name: `Batch Group ${i}`,
      }, token);
      expect(res.status).toBe(200);
      groups.push((res.data.data as { id: string }).id);
    }

    const listRes = await req('GET', '/v4/group', undefined, token);
    const data = listRes.data.data as { list: unknown[] };
    expect(data.list.length).toBeGreaterThanOrEqual(5);

    for (const id of groups) {
      await req('DELETE', `/v4/group/${id}`, undefined, token);
    }
  });

  it('should handle concurrent operations', async () => {
    const createRes = await req('POST', '/v4/group', {
      name: 'Concurrent Test Group',
    }, token);
    const gId = (createRes.data.data as { id: string }).id;

    const promises = [];
    for (let i = 0; i < 3; i++) {
      promises.push(req('GET', `/v4/group/${gId}`, undefined, token));
    }
    const results = await Promise.all(promises);
    for (const r of results) {
      expect(r.status).toBe(200);
    }

    await req('DELETE', `/v4/group/${gId}`, undefined, token);
  });
});

describe('Room API', () => {
  let ownerToken: string;
  let memberToken: string;
  let groupId: string;

  beforeAll(async () => {
    ownerToken = await registerUser('room_owner', 'password123');
    memberToken = await registerUser('room_member', 'password123');

    const createRes = await req('POST', '/v4/group', {
      name: 'Room Test Group',
      visibility: 'private',
    }, ownerToken);
    groupId = (createRes.data.data as { id: string }).id;

    await req('POST', `/v4/group/invite`, {
      group_id: groupId,
      user_id: ['room_member'],
      level: 0,
    }, ownerToken);
  });

  it('should search room by group_id', async () => {
    const res = await req('GET', `/v4/room?group_id=${groupId}`, undefined, ownerToken);
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
  });

  it('should search room by user_id', async () => {
    const res = await req('GET', `/v4/room?user_id=room_member`, undefined, ownerToken);
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
  });

  it('should update room level', async () => {
    const res = await req('PATCH', '/v4/room', {
      group_id: groupId,
      user_id: 'room_member',
      level: 1,
    }, ownerToken);
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
  });

  it('should remove room', async () => {
    const res = await req('DELETE', `/v4/room?group_id=${groupId}&user_id=room_member`, undefined, ownerToken);
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
  });
});

describe('SSO + Env API - Existing Functionality', () => {
  let token: string;

  beforeAll(async () => {
    token = await registerUser('sso_test_user', 'password123');
  });

  it('should check auth with valid token', async () => {
    const res = await req('GET', `/v4/sso/check`, undefined, token);
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
    expect((res.data.data as { user_id: string }).user_id).toBe('sso_test_user');
  });

  it('should get user info', async () => {
    const res = await req('GET', '/v4/sso/info', undefined, token);
    expect(res.status).toBe(200);
    expect((res.data.data as { user_id: string }).user_id).toBe('sso_test_user');
  });

  it('should login with correct credentials', async () => {
    const res = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    expect(res.status).toBe(200);
    expect(res.data.code).toBe(0);
    expect((res.data.data as { token: string }).token).toBeDefined();
  });

  it('should reject login with wrong password', async () => {
    const res = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'wrongpass',
    });
    expect(res.status).toBe(401);
  });

  it('should reject duplicate registration', async () => {
    const res = await req('POST', '/v4/sso/register', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    expect(res.status).toBe(409);
  });

  it('should create and read env', async () => {
    const freshLogin = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    const freshToken = (freshLogin.data.data as { token: string }).token;

    const createRes = await req('POST', '/v4/env', {
      user_id: 'sso_test_user',
      name: 'test-env',
      rule: 'example.com -> 127.0.0.1',
    }, freshToken);
    expect(createRes.status).toBe(200);
    const env = createRes.data.data as { id: string; name: string };
    expect(env.name).toBe('test-env');

    const readRes = await req('GET', `/v4/env/${env.id}`, undefined, freshToken);
    expect(readRes.status).toBe(200);
    expect((readRes.data.data as { name: string }).name).toBe('test-env');
  });

  it('should search envs', async () => {
    const freshLogin = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    const freshToken = (freshLogin.data.data as { token: string }).token;

    const res = await req('GET', '/v4/env?user_id=sso_test_user', undefined, freshToken);
    expect(res.status).toBe(200);
    const data = res.data.data as { list: unknown[] };
    expect(data.list.length).toBeGreaterThanOrEqual(1);
  });

  it('should update env', async () => {
    const freshLogin = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    const freshToken = (freshLogin.data.data as { token: string }).token;

    const searchRes = await req('GET', '/v4/env?user_id=sso_test_user', undefined, freshToken);
    const envs = (searchRes.data.data as { list: { id: string }[] }).list;
    expect(envs.length).toBeGreaterThan(0);

    const envId = envs[0].id;
    const res = await req('PATCH', `/v4/env/${envId}`, {
      rule: 'example.com -> 192.168.1.1',
    }, freshToken);
    expect(res.status).toBe(200);
  });

  it('should delete env', async () => {
    const freshLogin = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    const freshToken = (freshLogin.data.data as { token: string }).token;

    const searchRes = await req('GET', '/v4/env?user_id=sso_test_user', undefined, freshToken);
    const envs = (searchRes.data.data as { list: { id: string }[] }).list;
    const envId = envs[0].id;

    const res = await req('DELETE', `/v4/env/${envId}`, undefined, freshToken);
    expect(res.status).toBe(200);

    const readRes = await req('GET', `/v4/env/${envId}`, undefined, freshToken);
    expect(readRes.status).toBe(404);
  });

  it('should handle logout', async () => {
    const loginRes = await req('POST', '/v4/sso/login', {
      user_id: 'sso_test_user',
      password: 'password123',
    });
    const tempToken = (loginRes.data.data as { token: string }).token;

    const logoutRes = await req('GET', '/v4/sso/logout', undefined, tempToken);
    expect(logoutRes.status).toBe(200);

    const checkRes = await req('GET', '/v4/sso/check', undefined, tempToken);
    expect(checkRes.status).toBe(401);
  });
});

describe('Group Env Management - Permission & Rules', () => {
  let ownerToken: string;
  let masterToken: string;
  let memberToken: string;
  let outsiderToken: string;
  let groupId: string;
  const groupName = 'EnvTestGroup';

  beforeAll(async () => {
    ownerToken = await registerUser('env_grp_owner', 'password123');
    masterToken = await registerUser('env_grp_master', 'password123');
    memberToken = await registerUser('env_grp_member', 'password123');
    outsiderToken = await registerUser('env_grp_outsider', 'password123');

    const createRes = await req('POST', '/v4/group', {
      name: groupName,
      description: 'Group for env permission testing',
      visibility: 'private',
    }, ownerToken);
    groupId = (createRes.data.data as { id: string }).id;

    await req('POST', `/v4/group/invite`, {
      group_id: groupId,
      user_id: ['env_grp_master'],
      level: 1,
    }, ownerToken);

    await req('POST', `/v4/group/invite`, {
      group_id: groupId,
      user_id: ['env_grp_member'],
      level: 0,
    }, ownerToken);
  });

  describe('Create Env for Group', () => {
    it('owner should create env for group', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: groupName,
        name: 'production',
        rule: 'example.com -> 10.0.0.1',
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
      const env = res.data.data as { user_id: string; name: string };
      expect(env.user_id).toBe(groupName);
      expect(env.name).toBe('production');
    });

    it('master should create env for group', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: groupName,
        name: 'staging',
        rule: 'example.com -> 10.0.0.2',
      }, masterToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('regular member should NOT create env for group', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: groupName,
        name: 'dev',
        rule: 'example.com -> 10.0.0.3',
      }, memberToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT create env for group', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: groupName,
        name: 'hack',
        rule: 'evil.com -> 127.0.0.1',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('user can create env for themselves', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: 'env_grp_owner',
        name: 'my-env',
        rule: 'test.com -> 127.0.0.1',
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });
  });

  describe('Search Env for Group', () => {
    it('owner should search group envs', async () => {
      const res = await req('GET', `/v4/env?user_id=${groupName}`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { name: string }[] };
      expect(data.list.length).toBe(2);
    });

    it('member should search group envs', async () => {
      const res = await req('GET', `/v4/env?user_id=${groupName}`, undefined, memberToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[] };
      expect(data.list.length).toBe(2);
    });

    it('outsider should NOT search private group envs', async () => {
      const res = await req('GET', `/v4/env?user_id=${groupName}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('user can search their own envs without specifying user_id', async () => {
      const res = await req('GET', '/v4/env', undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: { user_id: string }[] };
      expect(data.list.every((e) => e.user_id === 'env_grp_owner')).toBe(true);
    });
  });

  describe('Read Env for Group', () => {
    let envId: string;

    beforeAll(async () => {
      const res = await req('GET', `/v4/env?user_id=${groupName}`, undefined, ownerToken);
      envId = (res.data.data as { list: { id: string }[] }).list[0].id;
    });

    it('owner should read group env', async () => {
      const res = await req('GET', `/v4/env/${envId}`, undefined, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('member should read group env', async () => {
      const res = await req('GET', `/v4/env/${envId}`, undefined, memberToken);
      expect(res.status).toBe(200);
    });

    it('outsider should NOT read private group env', async () => {
      const res = await req('GET', `/v4/env/${envId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Update Env for Group', () => {
    let envId: string;

    beforeAll(async () => {
      const res = await req('GET', `/v4/env?user_id=${groupName}`, undefined, ownerToken);
      envId = (res.data.data as { list: { id: string }[] }).list[0].id;
    });

    it('owner should update group env', async () => {
      const res = await req('PATCH', `/v4/env/${envId}`, {
        rule: 'example.com -> 10.0.0.100',
      }, ownerToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('master should update group env', async () => {
      const res = await req('PATCH', `/v4/env/${envId}`, {
        rule: 'example.com -> 10.0.0.200',
      }, masterToken);
      expect(res.status).toBe(200);
    });

    it('regular member should NOT update group env', async () => {
      const res = await req('PATCH', `/v4/env/${envId}`, {
        rule: 'example.com -> hacked',
      }, memberToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT update group env', async () => {
      const res = await req('PATCH', `/v4/env/${envId}`, {
        rule: 'evil.com -> hacked',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Delete Env for Group', () => {
    let envId: string;

    beforeAll(async () => {
      const createRes = await req('POST', '/v4/env', {
        user_id: groupName,
        name: 'to-delete',
        rule: 'delete.me -> 127.0.0.1',
      }, ownerToken);
      envId = (createRes.data.data as { id: string }).id;
    });

    it('regular member should NOT delete group env', async () => {
      const res = await req('DELETE', `/v4/env/${envId}`, undefined, memberToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT delete group env', async () => {
      const res = await req('DELETE', `/v4/env/${envId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('master should delete group env', async () => {
      const res = await req('DELETE', `/v4/env/${envId}`, undefined, masterToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });
  });

  describe('Public Group Env Access', () => {
    let publicGroupId: string;
    const publicGroupName = 'PublicEnvGroup';
    let publicEnvId: string;

    beforeAll(async () => {
      const res = await req('POST', '/v4/group', {
        name: publicGroupName,
        visibility: 'public',
      }, ownerToken);
      publicGroupId = (res.data.data as { id: string }).id;

      await req('PATCH', `/v4/group/${publicGroupId}/setting`, {
        level: 1,
      }, ownerToken);

      const envRes = await req('POST', '/v4/env', {
        user_id: publicGroupName,
        name: 'public-rule',
        rule: 'public.com -> 10.0.0.1',
      }, ownerToken);
      publicEnvId = (envRes.data.data as { id: string }).id;
    });

    it('outsider should read env of public group', async () => {
      const res = await req('GET', `/v4/env/${publicEnvId}`, undefined, outsiderToken);
      expect(res.status).toBe(200);
      expect(res.data.code).toBe(0);
    });

    it('outsider should search envs of public group', async () => {
      const res = await req('GET', `/v4/env?user_id=${publicGroupName}`, undefined, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[] };
      expect(data.list.length).toBe(1);
    });

    it('outsider should NOT create env for public group', async () => {
      const res = await req('POST', '/v4/env', {
        user_id: publicGroupName,
        name: 'hack',
        rule: 'evil -> 127.0.0.1',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT update env of public group', async () => {
      const res = await req('PATCH', `/v4/env/${publicEnvId}`, {
        rule: 'hacked -> 127.0.0.1',
      }, outsiderToken);
      expect(res.status).toBe(403);
    });

    it('outsider should NOT delete env of public group', async () => {
      const res = await req('DELETE', `/v4/env/${publicEnvId}`, undefined, outsiderToken);
      expect(res.status).toBe(403);
    });
  });

  describe('Cascade Delete - Group Deletion Cleans Envs', () => {
    let cascadeGroupId: string;
    const cascadeGroupName = 'CascadeGroup';
    let envId1: string;
    let envId2: string;

    beforeAll(async () => {
      const res = await req('POST', '/v4/group', {
        name: cascadeGroupName,
      }, ownerToken);
      cascadeGroupId = (res.data.data as { id: string }).id;

      const env1 = await req('POST', '/v4/env', {
        user_id: cascadeGroupName,
        name: 'env-1',
        rule: 'a.com -> 1.1.1.1',
      }, ownerToken);
      envId1 = (env1.data.data as { id: string }).id;
      const env2 = await req('POST', '/v4/env', {
        user_id: cascadeGroupName,
        name: 'env-2',
        rule: 'b.com -> 2.2.2.2',
      }, ownerToken);
      envId2 = (env2.data.data as { id: string }).id;
    });

    it('should have envs before deletion', async () => {
      const res = await req('GET', `/v4/env?user_id=${cascadeGroupName}`, undefined, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { list: unknown[] };
      expect(data.list.length).toBe(2);
    });

    it('deleting group should cascade delete envs', async () => {
      const deleteRes = await req('DELETE', `/v4/group/${cascadeGroupId}`, undefined, ownerToken);
      expect(deleteRes.status).toBe(200);

      const env1Res = await req('GET', `/v4/env/${envId1}`, undefined, ownerToken);
      expect(env1Res.status).toBe(404);

      const env2Res = await req('GET', `/v4/env/${envId2}`, undefined, ownerToken);
      expect(env2Res.status).toBe(404);
    });
  });

  describe('Sync with Group Permissions', () => {
    let syncGroupId: string;
    const syncGroupName = 'SyncPermGroup';
    let syncEnvId: string;

    beforeAll(async () => {
      const res = await req('POST', '/v4/group', {
        name: syncGroupName,
      }, ownerToken);
      syncGroupId = (res.data.data as { id: string }).id;

      await req('POST', `/v4/group/invite`, {
        group_id: syncGroupId,
        user_id: ['env_grp_master'],
        level: 1,
      }, ownerToken);

      const envRes = await req('POST', '/v4/env', {
        user_id: syncGroupName,
        name: 'sync-env',
        rule: 'sync.com -> 10.0.0.1',
      }, ownerToken);
      syncEnvId = (envRes.data.data as { id: string }).id;
    });

    it('master can sync update group env', async () => {
      const res = await req('POST', '/v4/env/sync', {
        user_ids: [],
        check_list: [],
        update_list: [{
          user_id: syncGroupName,
          id: syncEnvId,
          name: 'sync-env',
          rule: 'sync.com -> 10.0.0.99',
          update_time: new Date().toISOString(),
        }],
        delete_list: [],
      }, masterToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { result_list: { status: number }[] };
      expect(data.result_list[0].status).toBe(0);
    });

    it('outsider cannot sync update group env', async () => {
      const res = await req('POST', '/v4/env/sync', {
        user_ids: [],
        check_list: [],
        update_list: [{
          user_id: syncGroupName,
          id: syncEnvId,
          name: 'sync-env',
          rule: 'hacked -> evil',
          update_time: new Date().toISOString(),
        }],
        delete_list: [],
      }, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { result_list: { status: number; msg?: string }[] };
      expect(data.result_list[0].status).toBe(1);
      expect(data.result_list[0].msg).toContain('denied');
    });

    it('outsider cannot sync delete group env', async () => {
      const res = await req('POST', '/v4/env/sync', {
        user_ids: [],
        check_list: [],
        update_list: [],
        delete_list: [{
          user_id: syncGroupName,
          id: syncEnvId,
          delete_time: new Date().toISOString(),
        }],
      }, outsiderToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { result_list: { status: number; msg?: string }[] };
      expect(data.result_list[0].status).toBe(1);
      expect(data.result_list[0].msg).toContain('denied');
    });

    it('owner can sync delete group env', async () => {
      const res = await req('POST', '/v4/env/sync', {
        user_ids: [],
        check_list: [],
        update_list: [],
        delete_list: [{
          user_id: syncGroupName,
          id: syncEnvId,
          delete_time: new Date().toISOString(),
        }],
      }, ownerToken);
      expect(res.status).toBe(200);
      const data = res.data.data as { result_list: { status: number }[] };
      expect(data.result_list[0].status).toBe(0);
    });
  });
});

describe('Env sort_order Support', () => {
  let token: string;

  beforeAll(async () => {
    token = await registerUser('sort_order_user', 'password123');
  });

  it('should default sort_order to 0 when not provided', async () => {
    const res = await req('POST', '/v4/env', {
      user_id: 'sort_order_user',
      name: 'no-sort-order',
      rule: 'a.com -> 1.1.1.1',
    }, token);
    expect(res.status).toBe(200);
    const env = res.data.data as { id: string; sort_order: number };
    expect(env.sort_order).toBe(0);
  });

  it('should accept sort_order when creating env', async () => {
    const res = await req('POST', '/v4/env', {
      user_id: 'sort_order_user',
      name: 'with-sort-order',
      rule: 'b.com -> 2.2.2.2',
      sort_order: 5,
    }, token);
    expect(res.status).toBe(200);
    const env = res.data.data as { id: string; sort_order: number };
    expect(env.sort_order).toBe(5);
  });

  it('should return sort_order when reading env', async () => {
    const searchRes = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    const envs = (searchRes.data.data as { list: { id: string; name: string; sort_order: number }[] }).list;
    const env = envs.find(e => e.name === 'with-sort-order');
    expect(env).toBeDefined();

    const readRes = await req('GET', `/v4/env/${env!.id}`, undefined, token);
    expect(readRes.status).toBe(200);
    const detail = readRes.data.data as { sort_order: number };
    expect(detail.sort_order).toBe(5);
  });

  it('should return sort_order in search results', async () => {
    const res = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    expect(res.status).toBe(200);
    const data = res.data.data as { list: { name: string; sort_order: number }[] };
    const noSort = data.list.find(e => e.name === 'no-sort-order');
    const withSort = data.list.find(e => e.name === 'with-sort-order');
    expect(noSort!.sort_order).toBe(0);
    expect(withSort!.sort_order).toBe(5);
  });

  it('should update sort_order', async () => {
    const searchRes = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    const envs = (searchRes.data.data as { list: { id: string; name: string }[] }).list;
    const env = envs.find(e => e.name === 'no-sort-order')!;

    const updateRes = await req('PATCH', `/v4/env/${env.id}`, {
      sort_order: 10,
    }, token);
    expect(updateRes.status).toBe(200);
    const updated = updateRes.data.data as { sort_order: number };
    expect(updated.sort_order).toBe(10);
  });

  it('should preserve sort_order when not provided in update', async () => {
    const searchRes = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    const envs = (searchRes.data.data as { list: { id: string; name: string; sort_order: number }[] }).list;
    const env = envs.find(e => e.name === 'with-sort-order')!;
    expect(env.sort_order).toBe(5);

    const updateRes = await req('PATCH', `/v4/env/${env.id}`, {
      rule: 'b.com -> 3.3.3.3',
    }, token);
    expect(updateRes.status).toBe(200);
    const updated = updateRes.data.data as { sort_order: number };
    expect(updated.sort_order).toBe(5);
  });

  it('should handle sort_order in sync update_list for new env', async () => {
    const res = await req('POST', '/v4/env/sync', {
      user_ids: [],
      check_list: [],
      update_list: [{
        user_id: 'sort_order_user',
        id: 'sync-new-sort-id',
        name: 'sync-sort-env',
        rule: 'sync.com -> 5.5.5.5',
        sort_order: 3,
        update_time: new Date().toISOString(),
      }],
      delete_list: [],
    }, token);
    expect(res.status).toBe(200);
    const data = res.data.data as { result_list: { status: number; sort_order?: number; name?: string }[] };
    expect(data.result_list[0].status).toBe(0);
    expect(data.result_list[0].sort_order).toBe(3);
  });

  it('should handle sort_order in sync update_list for existing env', async () => {
    const searchRes = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    const envs = (searchRes.data.data as { list: { id: string; name: string; sort_order: number }[] }).list;
    const env = envs.find(e => e.name === 'sync-sort-env')!;
    expect(env.sort_order).toBe(3);

    const res = await req('POST', '/v4/env/sync', {
      user_ids: [],
      check_list: [],
      update_list: [{
        user_id: 'sort_order_user',
        id: env.id,
        name: 'sync-sort-env',
        rule: 'sync.com -> 6.6.6.6',
        sort_order: 7,
        update_time: new Date().toISOString(),
      }],
      delete_list: [],
    }, token);
    expect(res.status).toBe(200);
    const data = res.data.data as { result_list: { status: number; sort_order?: number }[] };
    expect(data.result_list[0].status).toBe(0);
    expect(data.result_list[0].sort_order).toBe(7);
  });

  it('should include sort_order in sync check_list local_update_list', async () => {
    const searchRes = await req('GET', '/v4/env?user_id=sort_order_user', undefined, token);
    const envs = (searchRes.data.data as { list: { id: string; name: string; sort_order: number; update_time: string }[] }).list;
    const env = envs.find(e => e.name === 'sync-sort-env')!;

    const res = await req('POST', '/v4/env/sync', {
      user_ids: [],
      check_list: [{
        id: env.id,
        user_id: 'sort_order_user',
        update_time: '1970-01-01T00:00:00.000Z',
        hash: '',
      }],
      update_list: [],
      delete_list: [],
    }, token);
    expect(res.status).toBe(200);
    const data = res.data.data as { local_update_list: { id: string; sort_order: number }[] };
    expect(data.local_update_list.length).toBe(1);
    expect(data.local_update_list[0].id).toBe(env.id);
    expect(data.local_update_list[0].sort_order).toBe(7);
  });
});
