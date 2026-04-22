import Database from 'better-sqlite3';
import crypto from 'crypto';
import path from 'path';
import fs from 'fs';
import { customAlphabet } from 'nanoid';

const nanoid = customAlphabet('0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_', 21);
import type {
  Env, User, CreateEnvReq, UpdateEnvReq, SearchEnvQuery,
  Group, GroupMember, GroupSetting, UpdateGroupReq, SearchGroupQuery, UpdateGroupSettingReq,
  RemoteInvokePairing, RemoteInvokeGrant, RemoteInvokeCall, RemoteInvokeEvent, RemoteInvokeClientRecord,
} from '../types';
import type { IUserDao, IEnvDao, IGroupDao, IGroupMemberDao, IGroupSettingDao, IRemoteInvokeDao, IStorage } from './types';

export class SqliteUserDao implements IUserDao {
  constructor(private db: Database.Database) { }

  async findByToken(token: string): Promise<User | undefined> {
    return this.db
      .prepare('SELECT * FROM bifrost_users WHERE token = ?')
      .get(token) as User | undefined;
  }

  async findByUserId(userId: string): Promise<User | undefined> {
    return this.db
      .prepare('SELECT * FROM bifrost_users WHERE user_id = ?')
      .get(userId) as User | undefined;
  }

  async register(
    userId: string,
    password: string,
    fields: Partial<Pick<User, 'nickname' | 'avatar' | 'email'>>,
  ): Promise<User> {
    const now = new Date().toISOString();
    const id = nanoid();
    const salt = crypto.randomBytes(16).toString('hex');
    const hash = crypto.scryptSync(password, salt, 64).toString('hex');
    const passwordHash = `${salt}:${hash}`;

    this.db
      .prepare(
        `INSERT INTO bifrost_users (id, user_id, nickname, avatar, email, password_hash, create_time, update_time)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(id, userId, fields.nickname ?? '', fields.avatar ?? '', fields.email ?? '', passwordHash, now, now);
    return (await this.findByUserId(userId))!;
  }

  async verifyPassword(userId: string, password: string): Promise<boolean> {
    const user = await this.findByUserId(userId);
    if (!user || !user.password_hash) return false;
    const [salt, storedHash] = user.password_hash.split(':');
    const hash = crypto.scryptSync(password, salt, 64).toString('hex');
    const a = Buffer.from(hash, 'hex');
    const b = Buffer.from(storedHash, 'hex');
    if (a.length !== b.length) return false;
    return crypto.timingSafeEqual(a, b);
  }

  async saveToken(userId: string, token: string): Promise<void> {
    this.db
      .prepare('UPDATE bifrost_users SET token = ?, update_time = ? WHERE user_id = ?')
      .run(token, new Date().toISOString(), userId);
  }

  async clearToken(userId: string): Promise<void> {
    this.db
      .prepare('UPDATE bifrost_users SET token = NULL, update_time = ? WHERE user_id = ?')
      .run(new Date().toISOString(), userId);
  }
}

export class SqliteEnvDao implements IEnvDao {
  constructor(private db: Database.Database) { }

  async findById(id: string): Promise<Env | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_envs WHERE id = ?').get(id) as Env | undefined;
  }

  async findByUserAndName(userId: string, name: string): Promise<Env | undefined> {
    return this.db
      .prepare('SELECT * FROM bifrost_envs WHERE user_id = ? AND name = ?')
      .get(userId, name) as Env | undefined;
  }

  async create(req: CreateEnvReq): Promise<Env> {
    const now = new Date().toISOString();
    const id = nanoid();
    this.db
      .prepare(
        'INSERT INTO bifrost_envs (id, user_id, name, rule, sort_order, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?)',
      )
      .run(id, req.user_id, req.name, req.rule ?? '', req.sort_order ?? 0, now, now);
    return (await this.findById(id))!;
  }

  async update(id: string, fields: UpdateEnvReq): Promise<Env | undefined> {
    const existing = await this.findById(id);
    if (!existing) return undefined;
    const now = new Date().toISOString();
    this.db
      .prepare('UPDATE bifrost_envs SET user_id = ?, name = ?, rule = ?, sort_order = ?, update_time = ? WHERE id = ?')
      .run(
        fields.user_id ?? existing.user_id,
        fields.name ?? existing.name,
        fields.rule ?? existing.rule,
        fields.sort_order ?? existing.sort_order,
        now,
        id,
      );
    return (await this.findById(id))!;
  }

  async delete(id: string): Promise<boolean> {
    const result = this.db.prepare('DELETE FROM bifrost_envs WHERE id = ?').run(id);
    return result.changes > 0;
  }

  async deleteByUserId(userId: string): Promise<number> {
    const result = this.db.prepare('DELETE FROM bifrost_envs WHERE user_id = ?').run(userId);
    return result.changes;
  }

  async search(query: SearchEnvQuery): Promise<{ list: Env[]; total: number }> {
    const conditions: string[] = [];
    const params: unknown[] = [];

    if (query.user_id) {
      const userIds = Array.isArray(query.user_id) ? query.user_id : [query.user_id];
      conditions.push(`user_id IN (${userIds.map(() => '?').join(', ')})`);
      params.push(...userIds);
    }
    if (query.keyword) {
      conditions.push('name LIKE ?');
      params.push(`%${query.keyword}%`);
    }

    const where = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 500;

    const countRow = this.db
      .prepare(`SELECT COUNT(*) as total FROM bifrost_envs ${where}`)
      .get(...params) as { total: number };
    const list = this.db
      .prepare(`SELECT * FROM bifrost_envs ${where} ORDER BY update_time DESC LIMIT ? OFFSET ?`)
      .all(...params, limit, offset) as Env[];

    return { list, total: countRow.total };
  }
}

export class SqliteGroupDao implements IGroupDao {
  constructor(private db: Database.Database) { }

  async create(
    name: string,
    avatar: string,
    description: string,
    visibility: string,
    createdBy: string,
  ): Promise<Group> {
    const now = new Date().toISOString();
    const id = nanoid();
    this.db
      .prepare(
        `INSERT INTO bifrost_groups (id, name, avatar, description, visibility, created_by, create_time, update_time)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(id, name, avatar, description, visibility, createdBy, now, now);
    return (await this.findById(id))!;
  }

  async findById(id: string): Promise<Group | undefined> {
    return this.db
      .prepare('SELECT * FROM bifrost_groups WHERE id = ?')
      .get(id) as Group | undefined;
  }

  async findByName(name: string): Promise<Group | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_groups WHERE name = ?').get(name) as Group | undefined;
  }

  async update(id: string, fields: UpdateGroupReq): Promise<Group | undefined> {
    const existing = await this.findById(id);
    if (!existing) return undefined;
    const now = new Date().toISOString();
    const sets: string[] = [];
    const params: unknown[] = [];
    if (fields.name !== undefined) {
      sets.push('name = ?');
      params.push(fields.name);
    }
    if (fields.avatar !== undefined) {
      sets.push('avatar = ?');
      params.push(fields.avatar);
    }
    if (fields.description !== undefined) {
      sets.push('description = ?');
      params.push(fields.description);
    }
    if (fields.visibility !== undefined) {
      sets.push('visibility = ?');
      params.push(fields.visibility);
    }
    if (sets.length === 0) return existing;
    sets.push('update_time = ?');
    params.push(now, id);
    this.db
      .prepare(`UPDATE bifrost_groups SET ${sets.join(', ')} WHERE id = ?`)
      .run(...params);
    return (await this.findById(id))!;
  }

  async delete(id: string): Promise<boolean> {
    this.db.prepare('DELETE FROM bifrost_group_members WHERE group_id = ?').run(id);
    this.db.prepare('DELETE FROM bifrost_group_settings WHERE group_id = ?').run(id);
    const result = this.db.prepare('DELETE FROM bifrost_groups WHERE id = ?').run(id);
    return result.changes > 0;
  }

  async search(
    query: SearchGroupQuery,
    userId?: string,
  ): Promise<{ list: Group[]; total: number }> {
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 500;
    const uid = query.user_id ?? userId;

    if (query.keyword) {
      const countRow = this.db
        .prepare(
          `SELECT COUNT(*) as total FROM bifrost_groups g
           WHERE g.name LIKE ?
           AND (g.visibility = 'public' OR EXISTS (
             SELECT 1 FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?
           ))`,
        )
        .get(`%${query.keyword}%`, uid ?? '') as { total: number };
      const list = this.db
        .prepare(
          `SELECT g.*, (SELECT m.level FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?) as level
           FROM bifrost_groups g
           WHERE g.name LIKE ?
           AND (g.visibility = 'public' OR EXISTS (
             SELECT 1 FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?
           ))
           ORDER BY g.update_time DESC LIMIT ? OFFSET ?`,
        )
        .all(uid ?? '', `%${query.keyword}%`, uid ?? '', limit, offset) as Group[];
      return { list, total: countRow.total };
    }

    if (uid) {
      const countRow = this.db
        .prepare(
          `SELECT COUNT(*) as total FROM bifrost_groups g
           INNER JOIN bifrost_group_members m ON g.id = m.group_id
           WHERE m.user_id = ?`,
        )
        .get(uid) as { total: number };
      const list = this.db
        .prepare(
          `SELECT g.*, m.level FROM bifrost_groups g
           INNER JOIN bifrost_group_members m ON g.id = m.group_id
           WHERE m.user_id = ?
           ORDER BY g.update_time DESC LIMIT ? OFFSET ?`,
        )
        .all(uid, limit, offset) as Group[];
      return { list, total: countRow.total };
    }

    const countRow = this.db
      .prepare('SELECT COUNT(*) as total FROM bifrost_groups')
      .get() as { total: number };
    const list = this.db
      .prepare('SELECT * FROM bifrost_groups ORDER BY update_time DESC LIMIT ? OFFSET ?')
      .all(limit, offset) as Group[];
    return { list, total: countRow.total };
  }
}

export class SqliteGroupMemberDao implements IGroupMemberDao {
  constructor(private db: Database.Database) { }

  async add(groupId: string, userId: string, level: number): Promise<GroupMember> {
    const now = new Date().toISOString();
    const id = nanoid();
    this.db
      .prepare(
        `INSERT INTO bifrost_group_members (id, group_id, user_id, level, create_time, update_time)
         VALUES (?, ?, ?, ?, ?, ?)`,
      )
      .run(id, groupId, userId, level, now, now);
    return (await this.findByGroupAndUser(groupId, userId))!;
  }

  async remove(groupId: string, userId: string): Promise<boolean> {
    const result = this.db
      .prepare('DELETE FROM bifrost_group_members WHERE group_id = ? AND user_id = ?')
      .run(groupId, userId);
    return result.changes > 0;
  }

  async updateLevel(groupId: string, userId: string, level: number): Promise<boolean> {
    const now = new Date().toISOString();
    const result = this.db
      .prepare('UPDATE bifrost_group_members SET level = ?, update_time = ? WHERE group_id = ? AND user_id = ?')
      .run(level, now, groupId, userId);
    return result.changes > 0;
  }

  async findByGroupAndUser(groupId: string, userId: string): Promise<GroupMember | undefined> {
    return this.db
      .prepare(
        `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
                u.nickname, u.avatar, u.email
         FROM bifrost_group_members m
         LEFT JOIN bifrost_users u ON m.user_id = u.user_id
         WHERE m.group_id = ? AND m.user_id = ?`,
      )
      .get(groupId, userId) as GroupMember | undefined;
  }

  async listByGroup(
    groupId: string,
    query?: { keyword?: string; offset?: number; limit?: number },
  ): Promise<{ list: GroupMember[]; total: number }> {
    const offset = query?.offset ?? 0;
    const limit = query?.limit ?? 500;
    const conditions: string[] = ['m.group_id = ?'];
    const params: unknown[] = [groupId];

    if (query?.keyword) {
      conditions.push('(m.user_id LIKE ? OR u.nickname LIKE ?)');
      params.push(`%${query.keyword}%`, `%${query.keyword}%`);
    }

    const where = conditions.join(' AND ');

    const countRow = this.db
      .prepare(
        `SELECT COUNT(*) as total FROM bifrost_group_members m
         LEFT JOIN bifrost_users u ON m.user_id = u.user_id
         WHERE ${where}`,
      )
      .get(...params) as { total: number };
    const list = this.db
      .prepare(
        `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
                u.nickname, u.avatar, u.email
         FROM bifrost_group_members m
         LEFT JOIN bifrost_users u ON m.user_id = u.user_id
         WHERE ${where}
         ORDER BY m.create_time ASC LIMIT ? OFFSET ?`,
      )
      .all(...params, limit, offset) as GroupMember[];

    return { list, total: countRow.total };
  }

  async listByUser(userId: string): Promise<GroupMember[]> {
    return this.db
      .prepare(
        `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
                u.nickname, u.avatar, u.email
         FROM bifrost_group_members m
         LEFT JOIN bifrost_users u ON m.user_id = u.user_id
         WHERE m.user_id = ?
         ORDER BY m.create_time ASC`,
      )
      .all(userId) as GroupMember[];
  }
}

export class SqliteGroupSettingDao implements IGroupSettingDao {
  constructor(private db: Database.Database) { }

  async init(groupId: string, visibility: string = 'private'): Promise<void> {
    this.db
      .prepare(
        `INSERT OR IGNORE INTO bifrost_group_settings (group_id, rules_enabled, visibility)
         VALUES (?, 1, ?)`,
      )
      .run(groupId, visibility);
  }

  async get(groupId: string): Promise<GroupSetting> {
    const row = this.db
      .prepare('SELECT * FROM bifrost_group_settings WHERE group_id = ?')
      .get(groupId) as GroupSetting | undefined;
    if (row) return row;
    return { group_id: groupId, rules_enabled: 1, visibility: 'private' };
  }

  async update(groupId: string, fields: UpdateGroupSettingReq): Promise<void> {
    const sets: string[] = [];
    const params: unknown[] = [];
    if (fields.rules_enabled !== undefined) {
      sets.push('rules_enabled = ?');
      params.push(fields.rules_enabled ? 1 : 0);
    }
    if (fields.visibility !== undefined) {
      sets.push('visibility = ?');
      params.push(fields.visibility);
    }
    if (sets.length === 0) return;
    params.push(groupId);
    this.db
      .prepare(`UPDATE bifrost_group_settings SET ${sets.join(', ')} WHERE group_id = ?`)
      .run(...params);
  }
}

export class SqliteRemoteInvokeDao implements IRemoteInvokeDao {
  constructor(private db: Database.Database) {}

  async createPairing(p: RemoteInvokePairing): Promise<RemoteInvokePairing> {
    this.db.prepare(
      `INSERT INTO bifrost_remote_invoke_pairings (id, user_id, client_instance_id, caller_fingerprint, pair_code, status, caller_pubkey, caller_ephemeral_pub, client_ephemeral_pub, caller_info_json, command_summary_json, command_json, relay_token, call_id, grant_id, expires_at, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(p.id, p.user_id, p.client_instance_id, p.caller_fingerprint, p.pair_code, p.status, p.caller_pubkey, p.caller_ephemeral_pub ?? '', p.client_ephemeral_pub ?? '', p.caller_info_json, p.command_summary_json, p.command_json, p.relay_token, p.call_id, p.grant_id, p.expires_at, p.create_time, p.update_time);
    return p;
  }

  async getPairing(pairingId: string): Promise<RemoteInvokePairing | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_pairings WHERE id = ?').get(pairingId) as RemoteInvokePairing | undefined;
  }

  async updatePairing(pairingId: string, fields: Partial<RemoteInvokePairing>): Promise<void> {
    const sets: string[] = [];
    const params: unknown[] = [];
    for (const [key, val] of Object.entries(fields)) {
      if (key === 'id') continue;
      sets.push(`${key} = ?`);
      params.push(val);
    }
    if (sets.length === 0) return;
    params.push(pairingId);
    this.db.prepare(`UPDATE bifrost_remote_invoke_pairings SET ${sets.join(', ')} WHERE id = ?`).run(...params);
  }

  async findPairingByCode(userId: string, clientInstanceId: string, pairCode: string): Promise<RemoteInvokePairing | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_pairings WHERE user_id = ? AND client_instance_id = ? AND pair_code = ? AND status = ? ORDER BY create_time DESC LIMIT 1').get(userId, clientInstanceId, pairCode, 'pending_approval') as RemoteInvokePairing | undefined;
  }

  async countPendingPairings(clientInstanceId: string): Promise<number> {
    const row = this.db.prepare('SELECT COUNT(*) as total FROM bifrost_remote_invoke_pairings WHERE client_instance_id = ? AND status = ?').get(clientInstanceId, 'pending_approval') as { total: number };
    return row.total;
  }

  async listPendingPairings(clientInstanceId: string): Promise<RemoteInvokePairing[]> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_pairings WHERE client_instance_id = ? AND status = ? ORDER BY create_time DESC').all(clientInstanceId, 'pending_approval') as RemoteInvokePairing[];
  }

  async cancelPendingPairings(clientInstanceId: string): Promise<number> {
    const result = this.db.prepare('UPDATE bifrost_remote_invoke_pairings SET status = ?, update_time = ? WHERE client_instance_id = ? AND status = ?').run('rejected', new Date().toISOString(), clientInstanceId, 'pending_approval');
    return result.changes;
  }

  async createGrant(g: RemoteInvokeGrant): Promise<RemoteInvokeGrant> {
    this.db.prepare(
      `INSERT INTO bifrost_remote_invoke_grants (id, user_id, client_instance_id, caller_fingerprint, caller_display_name, caller_ephemeral_pub, client_ephemeral_pub, grant_mode, grant_scope, status, first_authorized_at, expires_at, last_used_at, max_calls, remaining_calls, created_by, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(g.id, g.user_id, g.client_instance_id, g.caller_fingerprint, g.caller_display_name, g.caller_ephemeral_pub ?? '', g.client_ephemeral_pub ?? '', g.grant_mode, g.grant_scope, g.status, g.first_authorized_at, g.expires_at, g.last_used_at, g.max_calls, g.remaining_calls, g.created_by, g.update_time);
    return g;
  }

  async getGrant(grantId: string): Promise<RemoteInvokeGrant | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_grants WHERE id = ?').get(grantId) as RemoteInvokeGrant | undefined;
  }

  async findReusableGrant(userId: string, clientInstanceId: string, callerFingerprint: string): Promise<RemoteInvokeGrant | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_grants WHERE user_id = ? AND client_instance_id = ? AND caller_fingerprint = ? AND status = ? ORDER BY first_authorized_at DESC LIMIT 1').get(userId, clientInstanceId, callerFingerprint, 'active') as RemoteInvokeGrant | undefined;
  }

  async listGrants(userId: string, query: { client_instance_id?: string; status?: string; offset?: number; limit?: number }): Promise<{ list: RemoteInvokeGrant[]; total: number }> {
    const conditions: string[] = ['user_id = ?'];
    const params: unknown[] = [userId];
    if (query.client_instance_id) {
      conditions.push('client_instance_id = ?');
      params.push(query.client_instance_id);
    }
    if (query.status) {
      conditions.push('status = ?');
      params.push(query.status);
    }
    const where = conditions.join(' AND ');
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 100;
    const countRow = this.db.prepare(`SELECT COUNT(*) as total FROM bifrost_remote_invoke_grants WHERE ${where}`).get(...params) as { total: number };
    const list = this.db.prepare(`SELECT * FROM bifrost_remote_invoke_grants WHERE ${where} ORDER BY first_authorized_at DESC LIMIT ? OFFSET ?`).all(...params, limit, offset) as RemoteInvokeGrant[];
    return { list, total: countRow.total };
  }

  async countActiveGrantsForClient(clientInstanceId: string): Promise<number> {
    const row = this.db.prepare(
      'SELECT COUNT(*) as total FROM bifrost_remote_invoke_grants WHERE client_instance_id = ? AND status = ?'
    ).get(clientInstanceId, 'active') as { total: number };
    return row.total;
  }

  async listActiveGrantsForClient(clientInstanceId: string): Promise<RemoteInvokeGrant[]> {
    return this.db.prepare(
      'SELECT * FROM bifrost_remote_invoke_grants WHERE client_instance_id = ? AND status = ? ORDER BY first_authorized_at DESC'
    ).all(clientInstanceId, 'active') as RemoteInvokeGrant[];
  }

  async updateGrant(grantId: string, fields: Partial<RemoteInvokeGrant>): Promise<void> {
    const sets: string[] = [];
    const params: unknown[] = [];
    for (const [key, val] of Object.entries(fields)) {
      if (key === 'id') continue;
      sets.push(`${key} = ?`);
      params.push(val);
    }
    if (sets.length === 0) return;
    params.push(grantId);
    this.db.prepare(`UPDATE bifrost_remote_invoke_grants SET ${sets.join(', ')} WHERE id = ?`).run(...params);
  }

  async deleteGrant(grantId: string): Promise<boolean> {
    const result = this.db.prepare('DELETE FROM bifrost_remote_invoke_grants WHERE id = ?').run(grantId);
    return result.changes > 0;
  }

  async revokeSshGrantsForClient(clientInstanceId: string): Promise<number> {
    const now = new Date().toISOString();
    const result = this.db.prepare(
      `UPDATE bifrost_remote_invoke_grants
       SET status = ?, update_time = ?
       WHERE client_instance_id = ? AND status = ? AND created_by = ?`,
    ).run('removed', now, clientInstanceId, 'active', 'ssh_publickey');
    return result.changes;
  }

  async touchGrantLastUsed(grantId: string, ts: string): Promise<void> {
    this.db.prepare('UPDATE bifrost_remote_invoke_grants SET last_used_at = ?, update_time = ? WHERE id = ?').run(ts, ts, grantId);
  }

  async consumeGrantCall(grantId: string): Promise<void> {
    this.db.prepare('UPDATE bifrost_remote_invoke_grants SET remaining_calls = MAX(remaining_calls - 1, 0), update_time = ? WHERE id = ?').run(new Date().toISOString(), grantId);
  }

  async createCall(c: RemoteInvokeCall): Promise<RemoteInvokeCall> {
    this.db.prepare(
      `INSERT INTO bifrost_remote_invoke_calls (id, user_id, grant_id, pairing_id, client_instance_id, caller_fingerprint, source_ip, caller_display_name, status, command_summary_json, command_json, payload_digest, stdout_digest, stderr_digest, exit_code, started_at, ended_at, duration_ms, bytes_in, bytes_out) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(c.id, c.user_id, c.grant_id, c.pairing_id, c.client_instance_id, c.caller_fingerprint, c.source_ip, c.caller_display_name, c.status, c.command_summary_json, c.command_json, c.payload_digest, c.stdout_digest, c.stderr_digest, c.exit_code, c.started_at, c.ended_at, c.duration_ms, c.bytes_in, c.bytes_out);
    return c;
  }

  async getCall(callId: string): Promise<RemoteInvokeCall | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_calls WHERE id = ?').get(callId) as RemoteInvokeCall | undefined;
  }

  async updateCall(callId: string, fields: Partial<RemoteInvokeCall>): Promise<void> {
    const sets: string[] = [];
    const params: unknown[] = [];
    for (const [key, val] of Object.entries(fields)) {
      if (key === 'id') continue;
      sets.push(`${key} = ?`);
      params.push(val);
    }
    if (sets.length === 0) return;
    params.push(callId);
    this.db.prepare(`UPDATE bifrost_remote_invoke_calls SET ${sets.join(', ')} WHERE id = ?`).run(...params);
  }

  async listCalls(userId: string, query: { client_instance_id?: string; caller_fingerprint?: string; status?: string; offset?: number; limit?: number }): Promise<{ list: RemoteInvokeCall[]; total: number }> {
    const conditions: string[] = ['user_id = ?'];
    const params: unknown[] = [userId];
    if (query.client_instance_id) {
      conditions.push('client_instance_id = ?');
      params.push(query.client_instance_id);
    }
    if (query.caller_fingerprint) {
      conditions.push('caller_fingerprint = ?');
      params.push(query.caller_fingerprint);
    }
    if (query.status) {
      conditions.push('status = ?');
      params.push(query.status);
    }
    const where = conditions.join(' AND ');
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 100;
    const countRow = this.db.prepare(`SELECT COUNT(*) as total FROM bifrost_remote_invoke_calls WHERE ${where}`).get(...params) as { total: number };
    const list = this.db.prepare(`SELECT * FROM bifrost_remote_invoke_calls WHERE ${where} ORDER BY started_at DESC LIMIT ? OFFSET ?`).all(...params, limit, offset) as RemoteInvokeCall[];
    return { list, total: countRow.total };
  }

  async appendEvent(event: RemoteInvokeEvent): Promise<void> {
    this.db.prepare(
      `INSERT INTO bifrost_remote_invoke_events (id, call_id, event_type, seq, direction, event_summary_json, create_time) VALUES (?, ?, ?, ?, ?, ?, ?)`,
    ).run(event.id, event.call_id, event.event_type, event.seq, event.direction, event.event_summary_json, event.create_time);
  }

  async listCallEvents(callId: string, query?: { offset?: number; limit?: number }): Promise<{ list: RemoteInvokeEvent[]; total: number }> {
    const offset = query?.offset ?? 0;
    const limit = query?.limit ?? 500;
    const countRow = this.db.prepare('SELECT COUNT(*) as total FROM bifrost_remote_invoke_events WHERE call_id = ?').get(callId) as { total: number };
    const list = this.db.prepare('SELECT * FROM bifrost_remote_invoke_events WHERE call_id = ? ORDER BY create_time ASC LIMIT ? OFFSET ?').all(callId, limit, offset) as RemoteInvokeEvent[];
    return { list, total: countRow.total };
  }

  async registerClient(record: RemoteInvokeClientRecord): Promise<void> {
    this.db.prepare(
      `INSERT OR REPLACE INTO bifrost_remote_invoke_clients (client_instance_id, user_id, client_name, platform, bifrost_version, client_auth_token, client_pubkey_hash, token_expires_at, last_heartbeat_at, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(record.client_instance_id, record.user_id, record.client_name, record.platform, record.bifrost_version, record.client_auth_token, record.client_pubkey_hash, record.token_expires_at, record.last_heartbeat_at, record.create_time, record.update_time);
  }

  async getClientRecord(clientInstanceId: string): Promise<RemoteInvokeClientRecord | undefined> {
    return this.db.prepare('SELECT * FROM bifrost_remote_invoke_clients WHERE client_instance_id = ?').get(clientInstanceId) as RemoteInvokeClientRecord | undefined;
  }

  async updateClientRecord(clientInstanceId: string, fields: Partial<RemoteInvokeClientRecord>): Promise<void> {
    const sets: string[] = [];
    const params: unknown[] = [];
    for (const [key, val] of Object.entries(fields)) {
      if (key === 'client_instance_id') continue;
      sets.push(`${key} = ?`);
      params.push(val);
    }
    if (sets.length === 0) return;
    params.push(clientInstanceId);
    this.db.prepare(`UPDATE bifrost_remote_invoke_clients SET ${sets.join(', ')} WHERE client_instance_id = ?`).run(...params);
  }

  async cleanupExpiredData(now: string, retentionDays: number, maxRecords: number): Promise<number> {
    const cutoff = new Date(Date.now() - retentionDays * 24 * 60 * 60 * 1000).toISOString();
    let total = 0;

    const r1 = this.db.prepare('DELETE FROM bifrost_remote_invoke_events WHERE create_time < ?').run(cutoff);
    total += r1.changes;

    const r2 = this.db.prepare('DELETE FROM bifrost_remote_invoke_calls WHERE started_at < ?').run(cutoff);
    total += r2.changes;

    const r3 = this.db.prepare('DELETE FROM bifrost_remote_invoke_pairings WHERE create_time < ?').run(cutoff);
    total += r3.changes;

    const countRow = this.db.prepare('SELECT COUNT(*) as cnt FROM bifrost_remote_invoke_calls').get() as { cnt: number };
    if (countRow.cnt > maxRecords) {
      const excess = countRow.cnt - maxRecords;
      const r4 = this.db.prepare('DELETE FROM bifrost_remote_invoke_calls WHERE id IN (SELECT id FROM bifrost_remote_invoke_calls ORDER BY started_at ASC LIMIT ?)').run(excess);
      total += r4.changes;
    }

    return total;
  }
}

export class SqliteStorage implements IStorage {
  public user: SqliteUserDao;
  public env: SqliteEnvDao;
  public group: SqliteGroupDao;
  public groupMember: SqliteGroupMemberDao;
  public groupSetting: SqliteGroupSettingDao;
  public remoteInvoke: SqliteRemoteInvokeDao;
  private db: Database.Database;

  constructor(dataDir: string) {
    fs.mkdirSync(dataDir, { recursive: true });
    const dbPath = path.join(dataDir, 'bifrost-sync.db');
    this.db = new Database(dbPath);
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('foreign_keys = ON');
    this.migrate();
    this.user = new SqliteUserDao(this.db);
    this.env = new SqliteEnvDao(this.db);
    this.group = new SqliteGroupDao(this.db);
    this.groupMember = new SqliteGroupMemberDao(this.db);
    this.groupSetting = new SqliteGroupSettingDao(this.db);
    this.remoteInvoke = new SqliteRemoteInvokeDao(this.db);
  }

  private migrate() {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS bifrost_users (
        id            TEXT PRIMARY KEY,
        user_id       TEXT NOT NULL UNIQUE,
        nickname      TEXT NOT NULL DEFAULT '',
        avatar        TEXT NOT NULL DEFAULT '',
        email         TEXT NOT NULL DEFAULT '',
        password_hash TEXT NOT NULL DEFAULT '',
        token         TEXT,
        create_time   TEXT NOT NULL,
        update_time   TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS bifrost_envs (
        id          TEXT PRIMARY KEY,
        user_id     TEXT NOT NULL,
        name        TEXT NOT NULL,
        rule        TEXT NOT NULL DEFAULT '',
        sort_order  INTEGER NOT NULL DEFAULT 0,
        create_time TEXT NOT NULL,
        update_time TEXT NOT NULL,
        UNIQUE(user_id, name)
      );
      CREATE INDEX IF NOT EXISTS idx_bifrost_envs_user_id ON bifrost_envs(user_id);
      CREATE INDEX IF NOT EXISTS idx_bifrost_users_token  ON bifrost_users(token);
      CREATE TABLE IF NOT EXISTS bifrost_groups (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        avatar      TEXT DEFAULT '',
        description TEXT DEFAULT '',
        visibility  TEXT DEFAULT 'private',
        created_by  TEXT NOT NULL,
        create_time TEXT NOT NULL,
        update_time TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS bifrost_group_members (
        id          TEXT PRIMARY KEY,
        group_id    TEXT NOT NULL,
        user_id     TEXT NOT NULL,
        level       INTEGER DEFAULT 0,
        create_time TEXT NOT NULL,
        update_time TEXT NOT NULL,
        UNIQUE(group_id, user_id)
      );
      CREATE INDEX IF NOT EXISTS idx_bifrost_group_members_group_id ON bifrost_group_members(group_id);
      CREATE INDEX IF NOT EXISTS idx_bifrost_group_members_user_id  ON bifrost_group_members(user_id);
      CREATE TABLE IF NOT EXISTS bifrost_group_settings (
        group_id       TEXT PRIMARY KEY,
        rules_enabled  INTEGER DEFAULT 1,
        visibility     TEXT DEFAULT 'private'
      );
      CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_pairings (
        id                    TEXT PRIMARY KEY,
        user_id               TEXT NOT NULL,
        client_instance_id    TEXT NOT NULL,
        caller_fingerprint    TEXT NOT NULL DEFAULT '',
        pair_code             TEXT NOT NULL DEFAULT '',
        status                TEXT NOT NULL DEFAULT 'created',
        caller_pubkey         TEXT NOT NULL DEFAULT '',
        caller_ephemeral_pub  TEXT NOT NULL DEFAULT '',
        client_ephemeral_pub  TEXT NOT NULL DEFAULT '',
        caller_info_json      TEXT NOT NULL DEFAULT '{}',
        command_summary_json  TEXT NOT NULL DEFAULT '{}',
        command_json          TEXT NOT NULL DEFAULT '{}',
        relay_token           TEXT NOT NULL DEFAULT '',
        call_id               TEXT NOT NULL DEFAULT '',
        grant_id              TEXT NOT NULL DEFAULT '',
        expires_at            TEXT NOT NULL DEFAULT '',
        create_time           TEXT NOT NULL,
        update_time           TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_ri_pairings_user_code ON bifrost_remote_invoke_pairings(user_id, pair_code, status);
      CREATE INDEX IF NOT EXISTS idx_ri_pairings_client ON bifrost_remote_invoke_pairings(client_instance_id, status);
      CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_grants (
        id                    TEXT PRIMARY KEY,
        user_id               TEXT NOT NULL,
        client_instance_id    TEXT NOT NULL,
        caller_fingerprint    TEXT NOT NULL DEFAULT '',
        caller_display_name   TEXT NOT NULL DEFAULT '',
        caller_ephemeral_pub  TEXT NOT NULL DEFAULT '',
        client_ephemeral_pub  TEXT NOT NULL DEFAULT '',
        grant_mode            TEXT NOT NULL DEFAULT 'once',
        grant_scope           TEXT NOT NULL DEFAULT 'remote_query',
        status                TEXT NOT NULL DEFAULT 'active',
        first_authorized_at   TEXT NOT NULL DEFAULT '',
        expires_at            TEXT NOT NULL DEFAULT '',
        last_used_at          TEXT NOT NULL DEFAULT '',
        max_calls             INTEGER NOT NULL DEFAULT 1,
        remaining_calls       INTEGER NOT NULL DEFAULT 1,
        created_by            TEXT NOT NULL DEFAULT '',
        update_time           TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_ri_grants_reusable ON bifrost_remote_invoke_grants(user_id, client_instance_id, caller_fingerprint, status);
      CREATE INDEX IF NOT EXISTS idx_ri_grants_user ON bifrost_remote_invoke_grants(user_id, status, expires_at);
      CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_calls (
        id                    TEXT PRIMARY KEY,
        user_id               TEXT NOT NULL,
        grant_id              TEXT NOT NULL DEFAULT '',
        pairing_id            TEXT NOT NULL DEFAULT '',
        client_instance_id    TEXT NOT NULL DEFAULT '',
        caller_fingerprint    TEXT NOT NULL DEFAULT '',
        source_ip             TEXT NOT NULL DEFAULT '',
        caller_display_name   TEXT NOT NULL DEFAULT '',
        status                TEXT NOT NULL DEFAULT 'pending',
        command_summary_json  TEXT NOT NULL DEFAULT '{}',
        command_json          TEXT NOT NULL DEFAULT '{}',
        payload_digest        TEXT NOT NULL DEFAULT '',
        stdout_digest         TEXT NOT NULL DEFAULT '',
        stderr_digest         TEXT NOT NULL DEFAULT '',
        exit_code             INTEGER NOT NULL DEFAULT -1,
        started_at            TEXT NOT NULL DEFAULT '',
        ended_at              TEXT NOT NULL DEFAULT '',
        duration_ms           INTEGER NOT NULL DEFAULT 0,
        bytes_in              INTEGER NOT NULL DEFAULT 0,
        bytes_out             INTEGER NOT NULL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS idx_ri_calls_user ON bifrost_remote_invoke_calls(user_id, started_at);
      CREATE INDEX IF NOT EXISTS idx_ri_calls_grant ON bifrost_remote_invoke_calls(grant_id);
      CREATE INDEX IF NOT EXISTS idx_ri_calls_status ON bifrost_remote_invoke_calls(status, started_at);
      CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_events (
        id                    TEXT PRIMARY KEY,
        call_id               TEXT NOT NULL DEFAULT '',
        event_type            TEXT NOT NULL DEFAULT '',
        seq                   INTEGER NOT NULL DEFAULT 0,
        direction             TEXT NOT NULL DEFAULT '',
        event_summary_json    TEXT NOT NULL DEFAULT '{}',
        create_time           TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS idx_ri_events_call ON bifrost_remote_invoke_events(call_id, create_time);
      CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_clients (
        client_instance_id    TEXT PRIMARY KEY,
        user_id               TEXT NOT NULL DEFAULT '',
        client_name           TEXT NOT NULL DEFAULT '',
        platform              TEXT NOT NULL DEFAULT '',
        bifrost_version       TEXT NOT NULL DEFAULT '',
        client_auth_token     TEXT NOT NULL DEFAULT '',
        client_pubkey_hash    TEXT NOT NULL DEFAULT '',
        token_expires_at      TEXT NOT NULL DEFAULT '',
        last_heartbeat_at     TEXT NOT NULL DEFAULT '',
        create_time           TEXT NOT NULL,
        update_time           TEXT NOT NULL
      );
    `);
    this.migrateAddSortOrder();
  }

  private migrateAddSortOrder() {
    const columns = this.db.pragma('table_info(bifrost_envs)') as Array<{ name: string }>;
    const hasSortOrder = columns.some(col => col.name === 'sort_order');
    if (!hasSortOrder) {
      this.db.exec('ALTER TABLE bifrost_envs ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0');
    }
  }

  async close(): Promise<void> {
    this.db.close();
  }
}
