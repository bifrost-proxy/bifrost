import crypto from 'crypto';
import mysql, { type ExecuteValues, type Pool, type RowDataPacket, type ResultSetHeader } from 'mysql2/promise';
import { nanoid } from 'nanoid';
import type {
  Env, User, CreateEnvReq, UpdateEnvReq, SearchEnvQuery, MysqlConfig,
  BasicConfig, BasicConfigKey, UpsertBasicConfigReq,
  Group, GroupMember, GroupSetting, UpdateGroupReq, SearchGroupQuery, UpdateGroupSettingReq,
  RemoteInvokePairing, RemoteInvokeGrant, RemoteInvokeCall, RemoteInvokeEvent, RemoteInvokeClientRecord, RemoteInvokeSshClaim,
} from '../types';
import type { IUserDao, IEnvDao, IBasicConfigDao, IStorage, IGroupDao, IGroupMemberDao, IGroupSettingDao, IRemoteInvokeDao } from './types';

function rowToUser(row: RowDataPacket): User {
  return {
    id: row.id,
    user_id: row.user_id,
    nickname: row.nickname,
    avatar: row.avatar,
    email: row.email,
    password_hash: row.password_hash,
    token: row.token,
    create_time: row.create_time,
    update_time: row.update_time,
  };
}

function rowToEnv(row: RowDataPacket): Env {
  return {
    id: row.id,
    user_id: row.user_id,
    name: row.name,
    rule: row.rule,
    sort_order: row.sort_order ?? 0,
    create_time: row.create_time,
    update_time: row.update_time,
  };
}

function rowToBasicConfig(row: RowDataPacket): BasicConfig {
  return {
    id: row.id,
    user_id: row.user_id,
    config_key: row.config_key,
    value_json: row.value_json,
    hash: row.hash,
    create_time: row.create_time,
    update_time: row.update_time,
  };
}

function rowToGroup(row: RowDataPacket): Group {
  return {
    id: row.id,
    name: row.name,
    avatar: row.avatar,
    description: row.description,
    visibility: row.visibility,
    created_by: row.created_by,
    create_time: row.create_time,
    update_time: row.update_time,
  };
}

function rowToGroupMember(row: RowDataPacket): GroupMember {
  return {
    id: row.id,
    group_id: row.group_id,
    user_id: row.user_id,
    level: row.level,
    nickname: row.nickname,
    avatar: row.avatar,
    email: row.email,
    create_time: row.create_time,
    update_time: row.update_time,
  };
}

function rowToGroupSetting(row: RowDataPacket): GroupSetting {
  return {
    group_id: row.group_id,
    rules_enabled: row.rules_enabled,
    visibility: row.visibility,
  };
}

const REQUIRED_REMOTE_INVOKE_GRANT_COLUMNS = [
  'ssh_key_id',
  'ssh_key_fingerprint',
  'file_access',
  'caller_pubkey',
  'caller_pubkey_fp',
  'session_token_hash',
  'session_token_expires_at',
  'last_nonce_seen',
  'revoked_at',
];

const REQUIRED_REMOTE_INVOKE_PAIRING_COLUMNS = [
  'watch_token_hash',
  'claim_token_hash',
  'claim_expires_at',
  'claimed_at',
  'caller_ephemeral_sig',
];

const FORBIDDEN_REMOTE_INVOKE_GRANT_COLUMNS = [
  'policy_binding',
  'shell_policy_set_version_snapshot',
  'interactive_allowed',
  'stdin_allowed',
];

export function remoteInvokeSchemaNeedsReset(
  grantColumns: string[],
  pairingColumns: string[],
  nonceTableExists: boolean,
  sshClaimsTableExists: boolean,
): boolean {
  return REQUIRED_REMOTE_INVOKE_GRANT_COLUMNS.some(name => !grantColumns.includes(name)) ||
    REQUIRED_REMOTE_INVOKE_PAIRING_COLUMNS.some(name => !pairingColumns.includes(name)) ||
    !nonceTableExists ||
    !sshClaimsTableExists ||
    FORBIDDEN_REMOTE_INVOKE_GRANT_COLUMNS.some(name => grantColumns.includes(name));
}

export class MysqlUserDao implements IUserDao {
  constructor(private pool: Pool) {}

  async findByToken(token: string): Promise<User | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_users WHERE token = ?',
      [token],
    );
    return rows.length > 0 ? rowToUser(rows[0]) : undefined;
  }

  async findByUserId(userId: string): Promise<User | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_users WHERE user_id = ?',
      [userId],
    );
    return rows.length > 0 ? rowToUser(rows[0]) : undefined;
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

    await this.pool.execute(
      `INSERT INTO bifrost_users (id, user_id, nickname, avatar, email, password_hash, create_time, update_time)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      [id, userId, fields.nickname ?? '', fields.avatar ?? '', fields.email ?? '', passwordHash, now, now],
    );
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
    await this.pool.execute(
      'UPDATE bifrost_users SET token = ?, update_time = ? WHERE user_id = ?',
      [token, new Date().toISOString(), userId],
    );
  }

  async clearToken(userId: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_users SET token = NULL, update_time = ? WHERE user_id = ?',
      [new Date().toISOString(), userId],
    );
  }
}

export class MysqlEnvDao implements IEnvDao {
  constructor(private pool: Pool) {}

  async findById(id: string): Promise<Env | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_envs WHERE id = ?',
      [id],
    );
    return rows.length > 0 ? rowToEnv(rows[0]) : undefined;
  }

  async findByUserAndName(userId: string, name: string): Promise<Env | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_envs WHERE user_id = ? AND name = ?',
      [userId, name],
    );
    return rows.length > 0 ? rowToEnv(rows[0]) : undefined;
  }

  async create(req: CreateEnvReq): Promise<Env> {
    const now = new Date().toISOString();
    const id = nanoid();
    await this.pool.execute(
      'INSERT INTO bifrost_envs (id, user_id, name, rule, sort_order, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?)',
      [id, req.user_id, req.name, req.rule ?? '', req.sort_order ?? 0, now, now],
    );
    return (await this.findById(id))!;
  }

  async update(id: string, fields: UpdateEnvReq): Promise<Env | undefined> {
    const existing = await this.findById(id);
    if (!existing) return undefined;
    const now = new Date().toISOString();
    await this.pool.execute(
      'UPDATE bifrost_envs SET user_id = ?, name = ?, rule = ?, sort_order = ?, update_time = ? WHERE id = ?',
      [fields.user_id ?? existing.user_id, fields.name ?? existing.name, fields.rule ?? existing.rule, fields.sort_order ?? existing.sort_order, now, id],
    );
    return (await this.findById(id))!;
  }

  async delete(id: string): Promise<boolean> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_envs WHERE id = ?',
      [id],
    );
    return result.affectedRows > 0;
  }

  async deleteByUserId(userId: string): Promise<number> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_envs WHERE user_id = ?',
      [userId],
    );
    return result.affectedRows;
  }

  async search(query: SearchEnvQuery): Promise<{ list: Env[]; total: number }> {
    const conditions: string[] = [];
    const params: (string | number)[] = [];

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

    const [countRows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT COUNT(*) as total FROM bifrost_envs ${where}`,
      params,
    );
    const total = countRows[0].total as number;

    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT * FROM bifrost_envs ${where} ORDER BY update_time DESC LIMIT ? OFFSET ?`,
      [...params, limit, offset],
    );

    return { list: rows.map(rowToEnv), total };
  }
}

export class MysqlBasicConfigDao implements IBasicConfigDao {
  private schemaReady: Promise<void>;

  constructor(private pool: Pool) {
    this.schemaReady = this.createTableIfNeeded();
  }

  ready(): Promise<void> {
    return this.schemaReady;
  }

  private async createTableIfNeeded(): Promise<void> {
    await this.pool.query(`
      CREATE TABLE IF NOT EXISTS bifrost_basic_configs (
        id          VARCHAR(192) NOT NULL PRIMARY KEY,
        user_id     VARCHAR(128) NOT NULL,
        config_key  VARCHAR(64)  NOT NULL,
        value_json  LONGTEXT     NOT NULL,
        hash        VARCHAR(128) NOT NULL DEFAULT '',
        create_time VARCHAR(32)  NOT NULL,
        update_time VARCHAR(32)  NOT NULL,
        UNIQUE KEY uk_bifrost_basic_config (user_id, config_key),
        KEY idx_bifrost_basic_configs_user_id (user_id)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    `);
  }

  async find(userId: string, configKey: BasicConfigKey): Promise<BasicConfig | undefined> {
    await this.ready();
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_basic_configs WHERE user_id = ? AND config_key = ?',
      [userId, configKey],
    );
    return rows.length > 0 ? rowToBasicConfig(rows[0]) : undefined;
  }

  async upsert(req: UpsertBasicConfigReq): Promise<BasicConfig> {
    await this.ready();
    const now = new Date().toISOString();
    const id = `${req.user_id}:${req.config_key}`;
    await this.pool.execute(
      `INSERT INTO bifrost_basic_configs (id, user_id, config_key, value_json, hash, create_time, update_time)
       VALUES (?, ?, ?, ?, ?, ?, ?)
       ON DUPLICATE KEY UPDATE value_json = VALUES(value_json), hash = VALUES(hash), update_time = VALUES(update_time)`,
      [id, req.user_id, req.config_key, req.value_json, req.hash ?? '', now, now],
    );
    return (await this.find(req.user_id, req.config_key))!;
  }

  async delete(userId: string, configKey: BasicConfigKey): Promise<boolean> {
    await this.ready();
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_basic_configs WHERE user_id = ? AND config_key = ?',
      [userId, configKey],
    );
    return result.affectedRows > 0;
  }

  async listByUser(userId: string): Promise<BasicConfig[]> {
    await this.ready();
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_basic_configs WHERE user_id = ? ORDER BY config_key ASC',
      [userId],
    );
    return rows.map(rowToBasicConfig);
  }
}

export class MysqlGroupDao implements IGroupDao {
  constructor(private pool: Pool) {}

  async create(
    name: string,
    avatar: string,
    description: string,
    visibility: string,
    createdBy: string,
  ): Promise<Group> {
    const now = new Date().toISOString();
    const id = nanoid();
    await this.pool.execute(
      `INSERT INTO bifrost_groups (id, name, avatar, description, visibility, created_by, create_time, update_time)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      [id, name, avatar, description, visibility, createdBy, now, now],
    );
    return (await this.findById(id))!;
  }

  async findById(id: string): Promise<Group | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_groups WHERE id = ?',
      [id],
    );
    return rows.length > 0 ? rowToGroup(rows[0]) : undefined;
  }

  async findByName(name: string): Promise<Group | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_groups WHERE name = ?',
      [name],
    );
    return rows.length > 0 ? rowToGroup(rows[0]) : undefined;
  }

  async update(id: string, fields: UpdateGroupReq): Promise<Group | undefined> {
    const existing = await this.findById(id);
    if (!existing) return undefined;
    const now = new Date().toISOString();
    const sets: string[] = [];
    const params: (string | number)[] = [];
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
    await this.pool.execute(
      `UPDATE bifrost_groups SET ${sets.join(', ')} WHERE id = ?`,
      params,
    );
    return (await this.findById(id))!;
  }

  async delete(id: string): Promise<boolean> {
    await this.pool.execute('DELETE FROM bifrost_group_members WHERE group_id = ?', [id]);
    await this.pool.execute('DELETE FROM bifrost_group_settings WHERE group_id = ?', [id]);
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_groups WHERE id = ?',
      [id],
    );
    return result.affectedRows > 0;
  }

  async search(
    query: SearchGroupQuery,
    userId?: string,
  ): Promise<{ list: Group[]; total: number }> {
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 500;
    const uid = query.user_id ?? userId;

    if (query.keyword) {
      const [countRows] = await this.pool.execute<RowDataPacket[]>(
        `SELECT COUNT(*) as total FROM bifrost_groups g
         WHERE g.name LIKE ?
         AND (g.visibility = 'public' OR EXISTS (
           SELECT 1 FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?
         ))`,
        [`%${query.keyword}%`, uid ?? ''],
      );
      const total = countRows[0].total as number;
      const [rows] = await this.pool.execute<RowDataPacket[]>(
        `SELECT g.*, (SELECT m.level FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?) as level
         FROM bifrost_groups g
         WHERE g.name LIKE ?
         AND (g.visibility = 'public' OR EXISTS (
           SELECT 1 FROM bifrost_group_members m WHERE m.group_id = g.id AND m.user_id = ?
         ))
         ORDER BY g.update_time DESC LIMIT ? OFFSET ?`,
        [uid ?? '', `%${query.keyword}%`, uid ?? '', limit, offset],
      );
      return { list: rows.map(rowToGroup), total };
    }

    if (uid) {
      const [countRows] = await this.pool.execute<RowDataPacket[]>(
        `SELECT COUNT(*) as total FROM bifrost_groups g
         INNER JOIN bifrost_group_members m ON g.id = m.group_id
         WHERE m.user_id = ?`,
        [uid],
      );
      const total = countRows[0].total as number;
      const [rows] = await this.pool.execute<RowDataPacket[]>(
        `SELECT g.*, m.level FROM bifrost_groups g
         INNER JOIN bifrost_group_members m ON g.id = m.group_id
         WHERE m.user_id = ?
         ORDER BY g.update_time DESC LIMIT ? OFFSET ?`,
        [uid, limit, offset],
      );
      return { list: rows.map(rowToGroup), total };
    }

    const [countRows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT COUNT(*) as total FROM bifrost_groups',
    );
    const total = countRows[0].total as number;
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_groups ORDER BY update_time DESC LIMIT ? OFFSET ?',
      [limit, offset],
    );
    return { list: rows.map(rowToGroup), total };
  }
}

export class MysqlGroupMemberDao implements IGroupMemberDao {
  constructor(private pool: Pool) {}

  async add(groupId: string, userId: string, level: number): Promise<GroupMember> {
    const now = new Date().toISOString();
    const id = nanoid();
    await this.pool.execute(
      `INSERT INTO bifrost_group_members (id, group_id, user_id, level, create_time, update_time)
       VALUES (?, ?, ?, ?, ?, ?)`,
      [id, groupId, userId, level, now, now],
    );
    return (await this.findByGroupAndUser(groupId, userId))!;
  }

  async remove(groupId: string, userId: string): Promise<boolean> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_group_members WHERE group_id = ? AND user_id = ?',
      [groupId, userId],
    );
    return result.affectedRows > 0;
  }

  async updateLevel(groupId: string, userId: string, level: number): Promise<boolean> {
    const now = new Date().toISOString();
    const [result] = await this.pool.execute<ResultSetHeader>(
      'UPDATE bifrost_group_members SET level = ?, update_time = ? WHERE group_id = ? AND user_id = ?',
      [level, now, groupId, userId],
    );
    return result.affectedRows > 0;
  }

  async findByGroupAndUser(groupId: string, userId: string): Promise<GroupMember | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
              u.nickname, u.avatar, u.email
       FROM bifrost_group_members m
       LEFT JOIN bifrost_users u ON m.user_id = u.user_id
       WHERE m.group_id = ? AND m.user_id = ?`,
      [groupId, userId],
    );
    return rows.length > 0 ? rowToGroupMember(rows[0]) : undefined;
  }

  async listByGroup(
    groupId: string,
    query?: { keyword?: string; offset?: number; limit?: number },
  ): Promise<{ list: GroupMember[]; total: number }> {
    const offset = query?.offset ?? 0;
    const limit = query?.limit ?? 500;
    const conditions: string[] = ['m.group_id = ?'];
    const params: (string | number)[] = [groupId];

    if (query?.keyword) {
      conditions.push('(m.user_id LIKE ? OR u.nickname LIKE ?)');
      params.push(`%${query.keyword}%`, `%${query.keyword}%`);
    }

    const where = conditions.join(' AND ');

    const [countRows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT COUNT(*) as total FROM bifrost_group_members m
       LEFT JOIN bifrost_users u ON m.user_id = u.user_id
       WHERE ${where}`,
      params,
    );
    const total = countRows[0].total as number;
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
              u.nickname, u.avatar, u.email
       FROM bifrost_group_members m
       LEFT JOIN bifrost_users u ON m.user_id = u.user_id
       WHERE ${where}
       ORDER BY m.create_time ASC LIMIT ? OFFSET ?`,
      [...params, limit, offset],
    );

    return { list: rows.map(rowToGroupMember), total };
  }

  async listByUser(userId: string): Promise<GroupMember[]> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT m.id, m.group_id, m.user_id, m.level, m.create_time, m.update_time,
              u.nickname, u.avatar, u.email
       FROM bifrost_group_members m
       LEFT JOIN bifrost_users u ON m.user_id = u.user_id
       WHERE m.user_id = ?
       ORDER BY m.create_time ASC`,
      [userId],
    );
    return rows.map(rowToGroupMember);
  }
}

export class MysqlGroupSettingDao implements IGroupSettingDao {
  constructor(private pool: Pool) {}

  async init(groupId: string, visibility: string = 'private'): Promise<void> {
    await this.pool.execute(
      `INSERT IGNORE INTO bifrost_group_settings (group_id, rules_enabled, visibility)
       VALUES (?, 1, ?)`,
      [groupId, visibility],
    );
  }

  async get(groupId: string): Promise<GroupSetting> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_group_settings WHERE group_id = ?',
      [groupId],
    );
    if (rows.length > 0) return rowToGroupSetting(rows[0]);
    return { group_id: groupId, rules_enabled: 1, visibility: 'private' };
  }

  async update(groupId: string, fields: UpdateGroupSettingReq): Promise<void> {
    const sets: string[] = [];
    const params: (string | number)[] = [];
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
    await this.pool.execute(
      `UPDATE bifrost_group_settings SET ${sets.join(', ')} WHERE group_id = ?`,
      params,
    );
  }
}

export class MysqlRemoteInvokeDao implements IRemoteInvokeDao {
  private schemaReady: Promise<void>;

  constructor(private pool: Pool) {
    this.schemaReady = this.resetRemoteInvokeSchemaIfNeeded();
  }

  ready(): Promise<void> {
    return this.schemaReady;
  }

  private async tableExists(table: string): Promise<boolean> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES
       WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?`,
      [table],
    );
    return rows.length > 0;
  }

  private async tableColumns(table: string): Promise<string[]> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS
       WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?`,
      [table],
    );
    return rows.map(row => String(row.COLUMN_NAME));
  }

  private async resetRemoteInvokeSchemaIfNeeded(): Promise<void> {
    const grantColumns = await this.tableColumns('bifrost_remote_invoke_grants');
    const pairingColumns = await this.tableColumns('bifrost_remote_invoke_pairings');
    const nonceTable = await this.tableExists('bifrost_remote_invoke_nonces');
    const sshClaimsTable = await this.tableExists('bifrost_remote_invoke_ssh_claims');
    if (!remoteInvokeSchemaNeedsReset(grantColumns, pairingColumns, nonceTable, sshClaimsTable)) {
      return;
    }

    for (const statement of [
      'DROP TABLE IF EXISTS bifrost_remote_invoke_events',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_calls',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_nonces',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_ssh_claims',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_grants',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_pairings',
      'DROP TABLE IF EXISTS bifrost_remote_invoke_clients',
    ]) {
      await this.pool.query(statement);
    }

    await this.createRemoteInvokeTables();
  }

  private async createRemoteInvokeTables(): Promise<void> {
    const statements = [
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_pairings (
        id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
        user_id               VARCHAR(128) NOT NULL,
        client_instance_id    VARCHAR(128) NOT NULL,
        caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
        pair_code             VARCHAR(32)  NOT NULL DEFAULT '',
        status                VARCHAR(32)  NOT NULL DEFAULT 'created',
        caller_pubkey         TEXT         NOT NULL,
        caller_ephemeral_pub  TEXT         NOT NULL,
        caller_ephemeral_sig  TEXT         NOT NULL,
        client_ephemeral_pub  TEXT         NOT NULL,
        caller_info_json      LONGTEXT     NOT NULL,
        command_summary_json  LONGTEXT     NOT NULL,
        command_json          LONGTEXT     NOT NULL,
        relay_token           VARCHAR(128) NOT NULL DEFAULT '',
        call_id               VARCHAR(32)  NOT NULL DEFAULT '',
        grant_id              VARCHAR(32)  NOT NULL DEFAULT '',
        watch_token_hash      VARCHAR(128) NOT NULL DEFAULT '',
        claim_token_hash      VARCHAR(128) NOT NULL DEFAULT '',
        claim_expires_at      VARCHAR(32)  NOT NULL DEFAULT '',
        claimed_at            VARCHAR(32)  NOT NULL DEFAULT '',
        expires_at            VARCHAR(32)  NOT NULL DEFAULT '',
        create_time           VARCHAR(32)  NOT NULL,
        update_time           VARCHAR(32)  NOT NULL,
        KEY idx_ri_pairings_user_code (user_id, pair_code, status),
        KEY idx_ri_pairings_client (client_instance_id, status),
        KEY idx_ri_pairings_claim (claim_token_hash),
        KEY idx_ri_pairings_watch (watch_token_hash)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_grants (
        id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
        user_id               VARCHAR(128) NOT NULL,
        client_instance_id    VARCHAR(128) NOT NULL,
        caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
        caller_display_name   VARCHAR(255) NOT NULL DEFAULT '',
        caller_pubkey         TEXT         NOT NULL,
        caller_pubkey_fp      VARCHAR(128) NOT NULL DEFAULT '',
        caller_ephemeral_pub  TEXT         NOT NULL,
        client_ephemeral_pub  TEXT         NOT NULL,
        grant_mode            VARCHAR(32)  NOT NULL DEFAULT 'once',
        grant_scope           VARCHAR(64)  NOT NULL DEFAULT 'remote_query',
        file_access           VARCHAR(32)  NOT NULL DEFAULT 'none',
        ssh_key_id            VARCHAR(128) NOT NULL DEFAULT '',
        ssh_key_fingerprint   VARCHAR(128) NOT NULL DEFAULT '',
        status                VARCHAR(32)  NOT NULL DEFAULT 'active',
        first_authorized_at   VARCHAR(32)  NOT NULL DEFAULT '',
        expires_at            VARCHAR(32)  NOT NULL DEFAULT '',
        session_token_hash    VARCHAR(128) NOT NULL DEFAULT '',
        session_token_expires_at VARCHAR(32) NOT NULL DEFAULT '',
        last_nonce_seen       VARCHAR(128) NOT NULL DEFAULT '',
        revoked_at            VARCHAR(32)  NOT NULL DEFAULT '',
        last_used_at          VARCHAR(32)  NOT NULL DEFAULT '',
        max_calls             INT          NOT NULL DEFAULT 1,
        remaining_calls       INT          NOT NULL DEFAULT 1,
        created_by            VARCHAR(128) NOT NULL DEFAULT '',
        update_time           VARCHAR(32)  NOT NULL,
        KEY idx_ri_grants_reusable (user_id, client_instance_id, caller_fingerprint, status),
        KEY idx_ri_grants_user (user_id, status, expires_at),
        KEY idx_ri_grants_caller_fp (caller_pubkey_fp),
        KEY idx_ri_grants_session (session_token_hash)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_calls (
        id                    VARCHAR(32)  NOT NULL PRIMARY KEY,
        user_id               VARCHAR(128) NOT NULL,
        grant_id              VARCHAR(32)  NOT NULL DEFAULT '',
        pairing_id            VARCHAR(32)  NOT NULL DEFAULT '',
        client_instance_id    VARCHAR(128) NOT NULL DEFAULT '',
        caller_fingerprint    VARCHAR(128) NOT NULL DEFAULT '',
        source_ip             VARCHAR(64)  NOT NULL DEFAULT '',
        caller_display_name   VARCHAR(255) NOT NULL DEFAULT '',
        status                VARCHAR(32)  NOT NULL DEFAULT 'pending',
        command_summary_json  LONGTEXT     NOT NULL,
        command_json          LONGTEXT     NOT NULL,
        payload_digest        VARCHAR(128) NOT NULL DEFAULT '',
        stdout_digest         VARCHAR(128) NOT NULL DEFAULT '',
        stderr_digest         VARCHAR(128) NOT NULL DEFAULT '',
        exit_code             INT          NOT NULL DEFAULT -1,
        started_at            VARCHAR(32)  NOT NULL DEFAULT '',
        ended_at              VARCHAR(32)  NOT NULL DEFAULT '',
        duration_ms           INT          NOT NULL DEFAULT 0,
        bytes_in              INT          NOT NULL DEFAULT 0,
        bytes_out             INT          NOT NULL DEFAULT 0,
        KEY idx_ri_calls_user (user_id, started_at),
        KEY idx_ri_calls_grant (grant_id),
        KEY idx_ri_calls_status (status, started_at)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_events (
        id                    VARCHAR(32) NOT NULL PRIMARY KEY,
        call_id               VARCHAR(32) NOT NULL DEFAULT '',
        event_type            VARCHAR(64) NOT NULL DEFAULT '',
        seq                   INT         NOT NULL DEFAULT 0,
        direction             VARCHAR(32) NOT NULL DEFAULT '',
        event_summary_json    LONGTEXT    NOT NULL,
        create_time           VARCHAR(32) NOT NULL,
        KEY idx_ri_events_call (call_id, create_time)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_clients (
        client_instance_id    VARCHAR(128) NOT NULL PRIMARY KEY,
        user_id               VARCHAR(128) NOT NULL DEFAULT '',
        client_name           VARCHAR(255) NOT NULL DEFAULT '',
        platform              VARCHAR(64)  NOT NULL DEFAULT '',
        bifrost_version       VARCHAR(64)  NOT NULL DEFAULT '',
        client_auth_token     VARCHAR(128) NOT NULL DEFAULT '',
        client_pubkey_hash    VARCHAR(128) NOT NULL DEFAULT '',
        token_expires_at      VARCHAR(32)  NOT NULL DEFAULT '',
        last_heartbeat_at     VARCHAR(32)  NOT NULL DEFAULT '',
        create_time           VARCHAR(32)  NOT NULL,
        update_time           VARCHAR(32)  NOT NULL
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_nonces (
        caller_pubkey_fp      VARCHAR(128) NOT NULL,
        nonce                 VARCHAR(128) NOT NULL,
        seen_at               VARCHAR(32)  NOT NULL,
        PRIMARY KEY (caller_pubkey_fp, nonce),
        KEY idx_ri_nonces_seen (seen_at)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
      `CREATE TABLE IF NOT EXISTS bifrost_remote_invoke_ssh_claims (
        claim_token_hash      VARCHAR(128) NOT NULL PRIMARY KEY,
        grant_id              VARCHAR(64)  NOT NULL,
        client_instance_id    VARCHAR(128) NOT NULL DEFAULT '',
        caller_pubkey_fp      VARCHAR(128) NOT NULL DEFAULT '',
        expires_at            VARCHAR(32)  NOT NULL,
        create_time           VARCHAR(32)  NOT NULL,
        claimed_at            VARCHAR(32)  NOT NULL DEFAULT '',
        KEY idx_ri_ssh_claims_grant (grant_id),
        KEY idx_ri_ssh_claims_expires (expires_at)
      ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`,
    ];

    for (const statement of statements) {
      await this.pool.query(statement);
    }
  }

  async createPairing(p: RemoteInvokePairing): Promise<RemoteInvokePairing> {
    await this.pool.execute(
      `INSERT INTO bifrost_remote_invoke_pairings (id, user_id, client_instance_id, caller_fingerprint, pair_code, status, caller_pubkey, caller_ephemeral_pub, caller_ephemeral_sig, client_ephemeral_pub, caller_info_json, command_summary_json, command_json, relay_token, call_id, grant_id, watch_token_hash, claim_token_hash, claim_expires_at, claimed_at, expires_at, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      [p.id, p.user_id, p.client_instance_id, p.caller_fingerprint, p.pair_code, p.status, p.caller_pubkey, p.caller_ephemeral_pub ?? '', p.caller_ephemeral_sig ?? '', p.client_ephemeral_pub ?? '', p.caller_info_json, p.command_summary_json, p.command_json, p.relay_token, p.call_id, p.grant_id, p.watch_token_hash ?? '', p.claim_token_hash ?? '', p.claim_expires_at ?? '', p.claimed_at ?? '', p.expires_at, p.create_time, p.update_time],
    );
    return p;
  }

  async getPairing(pairingId: string): Promise<RemoteInvokePairing | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_pairings WHERE id = ?', [pairingId]);
    return rows[0] as RemoteInvokePairing | undefined;
  }

  async getPairingByClaimTokenHash(hash: string): Promise<RemoteInvokePairing | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_pairings WHERE claim_token_hash = ? LIMIT 1', [hash]);
    return rows[0] as RemoteInvokePairing | undefined;
  }

  async getPairingByWatchTokenHash(hash: string): Promise<RemoteInvokePairing | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_pairings WHERE watch_token_hash = ? LIMIT 1', [hash]);
    return rows[0] as RemoteInvokePairing | undefined;
  }

  async setPairingClaimTokens(pairingId: string, claimHash: string, watchHash: string, claimExpiresAt: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_pairings SET claim_token_hash = ?, watch_token_hash = ?, claim_expires_at = ?, update_time = ? WHERE id = ?',
      [claimHash, watchHash, claimExpiresAt, new Date().toISOString(), pairingId],
    );
  }

  async markPairingClaimed(pairingId: string, claimedAt: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_pairings SET claimed_at = ?, claim_token_hash = ?, update_time = ? WHERE id = ?',
      [claimedAt, '', claimedAt, pairingId],
    );
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
    await this.pool.execute(`UPDATE bifrost_remote_invoke_pairings SET ${sets.join(', ')} WHERE id = ?`, params as ExecuteValues[]);
  }

  async findPairingByCode(userId: string, clientInstanceId: string, pairCode: string): Promise<RemoteInvokePairing | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_pairings WHERE user_id = ? AND client_instance_id = ? AND pair_code = ? AND status = ? ORDER BY create_time DESC LIMIT 1',
      [userId, clientInstanceId, pairCode, 'pending_approval'],
    );
    return rows[0] as RemoteInvokePairing | undefined;
  }

  async countPendingPairings(clientInstanceId: string): Promise<number> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT COUNT(*) as total FROM bifrost_remote_invoke_pairings WHERE client_instance_id = ? AND status = ?',
      [clientInstanceId, 'pending_approval'],
    );
    return rows[0]?.total as number ?? 0;
  }

  async listPendingPairings(clientInstanceId: string): Promise<RemoteInvokePairing[]> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_pairings WHERE client_instance_id = ? AND status = ? ORDER BY create_time DESC',
      [clientInstanceId, 'pending_approval'],
    );
    return rows as RemoteInvokePairing[];
  }

  async cancelPendingPairings(clientInstanceId: string): Promise<number> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'UPDATE bifrost_remote_invoke_pairings SET status = ?, update_time = ? WHERE client_instance_id = ? AND status = ?',
      ['rejected', new Date().toISOString(), clientInstanceId, 'pending_approval'],
    );
    return result.affectedRows;
  }

  async createGrant(g: RemoteInvokeGrant): Promise<RemoteInvokeGrant> {
    const callerPubkeyFp = g.caller_pubkey_fp ?? g.caller_fingerprint;
    await this.pool.execute(
      `INSERT INTO bifrost_remote_invoke_grants (id, user_id, client_instance_id, caller_fingerprint, caller_display_name, caller_pubkey, caller_pubkey_fp, caller_ephemeral_pub, client_ephemeral_pub, grant_mode, grant_scope, file_access, ssh_key_id, ssh_key_fingerprint, status, first_authorized_at, expires_at, session_token_hash, session_token_expires_at, last_nonce_seen, revoked_at, last_used_at, max_calls, remaining_calls, created_by, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      [g.id, g.user_id, g.client_instance_id, g.caller_fingerprint, g.caller_display_name, g.caller_pubkey ?? '', callerPubkeyFp, g.caller_ephemeral_pub ?? '', g.client_ephemeral_pub ?? '', g.grant_mode, g.grant_scope, g.file_access ?? 'none', g.ssh_key_id ?? '', g.ssh_key_fingerprint ?? '', g.status, g.first_authorized_at, g.expires_at, g.session_token_hash ?? '', g.session_token_expires_at ?? '', g.last_nonce_seen ?? '', g.revoked_at ?? '', g.last_used_at, g.max_calls, g.remaining_calls, g.created_by, g.update_time],
    );
    return g;
  }

  async getGrant(grantId: string): Promise<RemoteInvokeGrant | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_grants WHERE id = ?', [grantId]);
    return rows[0] as RemoteInvokeGrant | undefined;
  }

  async getGrantByCallerFp(callerFp: string, clientInstanceId: string): Promise<RemoteInvokeGrant | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_grants WHERE client_instance_id = ? AND caller_pubkey_fp = ? AND status = ? ORDER BY first_authorized_at DESC LIMIT 1',
      [clientInstanceId, callerFp, 'active'],
    );
    return rows[0] as RemoteInvokeGrant | undefined;
  }

  async getGrantBySessionTokenHash(hash: string): Promise<RemoteInvokeGrant | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_grants WHERE session_token_hash = ? LIMIT 1',
      [hash],
    );
    return rows[0] as RemoteInvokeGrant | undefined;
  }

  async updateGrantCallerPubkey(grantId: string, pubkey: string, fp: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET caller_pubkey = ?, caller_pubkey_fp = ?, caller_fingerprint = ?, update_time = ? WHERE id = ?',
      [pubkey, fp, fp, new Date().toISOString(), grantId],
    );
  }

  async updateGrantCallerEphemeralPub(grantId: string, pub: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET caller_ephemeral_pub = ?, update_time = ? WHERE id = ?',
      [pub, new Date().toISOString(), grantId],
    );
  }

  async updateGrantClientEphemeralPub(grantId: string, pub: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET client_ephemeral_pub = ?, update_time = ? WHERE id = ?',
      [pub, new Date().toISOString(), grantId],
    );
  }

  async updateGrantSessionToken(grantId: string, hash: string, expiresAt: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET session_token_hash = ?, session_token_expires_at = ?, update_time = ? WHERE id = ?',
      [hash, expiresAt, new Date().toISOString(), grantId],
    );
  }

  async revokeGrant(grantId: string, revokedAt: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET status = ?, revoked_at = ?, session_token_hash = ?, update_time = ? WHERE id = ?',
      ['revoked', revokedAt, '', revokedAt, grantId],
    );
  }

  async markNonceUsed(callerFp: string, nonce: string, seenAt: string): Promise<boolean> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'INSERT IGNORE INTO bifrost_remote_invoke_nonces (caller_pubkey_fp, nonce, seen_at) VALUES (?, ?, ?)',
      [callerFp, nonce, seenAt],
    );
    if (result.affectedRows > 0) {
      await this.pool.execute(
        'UPDATE bifrost_remote_invoke_grants SET last_nonce_seen = ?, update_time = ? WHERE caller_pubkey_fp = ?',
        [nonce, seenAt, callerFp],
      );
      return true;
    }
    return false;
  }

  async gcNonces(before: string): Promise<number> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      'DELETE FROM bifrost_remote_invoke_nonces WHERE seen_at < ?',
      [before],
    );
    return result.affectedRows;
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
    const [countRows] = await this.pool.execute<RowDataPacket[]>(`SELECT COUNT(*) as total FROM bifrost_remote_invoke_grants WHERE ${where}`, params as ExecuteValues[]);
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT * FROM bifrost_remote_invoke_grants WHERE ${where} ORDER BY first_authorized_at DESC LIMIT ? OFFSET ?`,
      [...params, limit, offset] as ExecuteValues[],
    );
    return { list: rows as RemoteInvokeGrant[], total: countRows[0]?.total as number ?? 0 };
  }

  async countActiveGrantsForClient(clientInstanceId: string): Promise<number> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT COUNT(*) as total FROM bifrost_remote_invoke_grants WHERE client_instance_id = ? AND status = ?',
      [clientInstanceId, 'active'],
    );
    return rows[0]?.total as number ?? 0;
  }

  async listActiveGrantsForClient(clientInstanceId: string): Promise<RemoteInvokeGrant[]> {
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_grants WHERE client_instance_id = ? AND status = ? ORDER BY first_authorized_at DESC',
      [clientInstanceId, 'active'],
    );
    return rows as RemoteInvokeGrant[];
  }

  async revokeActiveGrantsForCaller(clientInstanceId: string, callerFingerprint: string, excludeGrantId?: string): Promise<number> {
    const now = new Date().toISOString();
    const params = excludeGrantId
      ? ['removed', now, clientInstanceId, callerFingerprint, 'active', excludeGrantId]
      : ['removed', now, clientInstanceId, callerFingerprint, 'active'];
    const suffix = excludeGrantId ? ' AND id != ?' : '';
    const [result] = await this.pool.execute<ResultSetHeader>(
      `UPDATE bifrost_remote_invoke_grants SET status = ?, update_time = ? WHERE client_instance_id = ? AND caller_fingerprint = ? AND status = ?${suffix}`,
      params,
    );
    return result.affectedRows;
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
    await this.pool.execute(`UPDATE bifrost_remote_invoke_grants SET ${sets.join(', ')} WHERE id = ?`, params as ExecuteValues[]);
  }

  async deleteGrant(grantId: string): Promise<boolean> {
    const [result] = await this.pool.execute<ResultSetHeader>('DELETE FROM bifrost_remote_invoke_grants WHERE id = ?', [grantId]);
    return result.affectedRows > 0;
  }

  async revokeSshGrantsForClient(clientInstanceId: string): Promise<number> {
    const now = new Date().toISOString();
    const [result] = await this.pool.execute<ResultSetHeader>(
      'UPDATE bifrost_remote_invoke_grants SET status = ?, update_time = ? WHERE client_instance_id = ? AND status = ? AND created_by = ?',
      ['removed', now, clientInstanceId, 'active', 'ssh_publickey'],
    );
    return result.affectedRows;
  }

  async touchGrantLastUsed(grantId: string, ts: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_grants SET last_used_at = ?, update_time = ? WHERE id = ?',
      [ts, ts, grantId],
    );
  }

  async consumeGrantCall(grantId: string): Promise<boolean> {
    const [result] = await this.pool.execute<ResultSetHeader>(
      `UPDATE bifrost_remote_invoke_grants
       SET remaining_calls = remaining_calls - 1, update_time = ?
       WHERE id = ? AND status = ? AND remaining_calls > 0`,
      [new Date().toISOString(), grantId, 'active'],
    );
    return result.affectedRows === 1;
  }

  async createCall(c: RemoteInvokeCall): Promise<RemoteInvokeCall> {
    await this.pool.execute(
      `INSERT INTO bifrost_remote_invoke_calls (id, user_id, grant_id, pairing_id, client_instance_id, caller_fingerprint, source_ip, caller_display_name, status, command_summary_json, command_json, payload_digest, stdout_digest, stderr_digest, exit_code, started_at, ended_at, duration_ms, bytes_in, bytes_out) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      [c.id, c.user_id, c.grant_id, c.pairing_id, c.client_instance_id, c.caller_fingerprint, c.source_ip, c.caller_display_name, c.status, c.command_summary_json, c.command_json, c.payload_digest, c.stdout_digest, c.stderr_digest, c.exit_code, c.started_at, c.ended_at, c.duration_ms, c.bytes_in, c.bytes_out],
    );
    return c;
  }

  async getCall(callId: string): Promise<RemoteInvokeCall | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_calls WHERE id = ?', [callId]);
    return rows[0] as RemoteInvokeCall | undefined;
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
    await this.pool.execute(`UPDATE bifrost_remote_invoke_calls SET ${sets.join(', ')} WHERE id = ?`, params as ExecuteValues[]);
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
    const [countRows] = await this.pool.execute<RowDataPacket[]>(`SELECT COUNT(*) as total FROM bifrost_remote_invoke_calls WHERE ${where}`, params as ExecuteValues[]);
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      `SELECT * FROM bifrost_remote_invoke_calls WHERE ${where} ORDER BY started_at DESC LIMIT ? OFFSET ?`,
      [...params, limit, offset] as ExecuteValues[],
    );
    return { list: rows as RemoteInvokeCall[], total: countRows[0]?.total as number ?? 0 };
  }

  async appendEvent(event: RemoteInvokeEvent): Promise<void> {
    await this.pool.execute(
      'INSERT INTO bifrost_remote_invoke_events (id, call_id, event_type, seq, direction, event_summary_json, create_time) VALUES (?, ?, ?, ?, ?, ?, ?)',
      [event.id, event.call_id, event.event_type, event.seq, event.direction, event.event_summary_json, event.create_time],
    );
  }

  async listCallEvents(callId: string, query?: { offset?: number; limit?: number }): Promise<{ list: RemoteInvokeEvent[]; total: number }> {
    const offset = query?.offset ?? 0;
    const limit = query?.limit ?? 500;
    const [countRows] = await this.pool.execute<RowDataPacket[]>('SELECT COUNT(*) as total FROM bifrost_remote_invoke_events WHERE call_id = ?', [callId]);
    const [rows] = await this.pool.execute<RowDataPacket[]>(
      'SELECT * FROM bifrost_remote_invoke_events WHERE call_id = ? ORDER BY create_time ASC LIMIT ? OFFSET ?',
      [callId, limit, offset],
    );
    return { list: rows as RemoteInvokeEvent[], total: countRows[0]?.total as number ?? 0 };
  }

  async registerClient(record: RemoteInvokeClientRecord): Promise<void> {
    await this.pool.execute(
      `INSERT INTO bifrost_remote_invoke_clients (client_instance_id, user_id, client_name, platform, bifrost_version, client_auth_token, client_pubkey_hash, token_expires_at, last_heartbeat_at, create_time, update_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON DUPLICATE KEY UPDATE user_id = VALUES(user_id), client_name = VALUES(client_name), platform = VALUES(platform), bifrost_version = VALUES(bifrost_version), client_auth_token = VALUES(client_auth_token), client_pubkey_hash = VALUES(client_pubkey_hash), token_expires_at = VALUES(token_expires_at), last_heartbeat_at = VALUES(last_heartbeat_at), update_time = VALUES(update_time)`,
      [record.client_instance_id, record.user_id, record.client_name, record.platform, record.bifrost_version, record.client_auth_token, record.client_pubkey_hash, record.token_expires_at, record.last_heartbeat_at, record.create_time, record.update_time],
    );
  }

  async getClientRecord(clientInstanceId: string): Promise<RemoteInvokeClientRecord | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_clients WHERE client_instance_id = ?', [clientInstanceId]);
    return rows[0] as RemoteInvokeClientRecord | undefined;
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
    await this.pool.execute(`UPDATE bifrost_remote_invoke_clients SET ${sets.join(', ')} WHERE client_instance_id = ?`, params as ExecuteValues[]);
  }

  async createSshClaim(claim: RemoteInvokeSshClaim): Promise<void> {
    await this.pool.execute(
      `INSERT INTO bifrost_remote_invoke_ssh_claims (claim_token_hash, grant_id, client_instance_id, caller_pubkey_fp, expires_at, create_time, claimed_at) VALUES (?, ?, ?, ?, ?, ?, ?)`,
      [claim.claim_token_hash, claim.grant_id, claim.client_instance_id, claim.caller_pubkey_fp, claim.expires_at, claim.create_time, claim.claimed_at ?? ''],
    );
  }

  async getSshClaimByTokenHash(hash: string): Promise<RemoteInvokeSshClaim | undefined> {
    const [rows] = await this.pool.execute<RowDataPacket[]>('SELECT * FROM bifrost_remote_invoke_ssh_claims WHERE claim_token_hash = ? LIMIT 1', [hash]);
    return rows[0] as RemoteInvokeSshClaim | undefined;
  }

  async markSshClaimRedeemed(hash: string, claimedAt: string): Promise<void> {
    await this.pool.execute(
      'UPDATE bifrost_remote_invoke_ssh_claims SET claimed_at = ? WHERE claim_token_hash = ?',
      [claimedAt, hash],
    );
  }

  async cleanupExpiredData(_now: string, retentionDays: number, maxRecords: number): Promise<number> {
    const cutoff = new Date(Date.now() - retentionDays * 24 * 60 * 60 * 1000).toISOString();
    let total = 0;

    for (const [sql, params] of [
      ['DELETE FROM bifrost_remote_invoke_events WHERE create_time < ?', [cutoff]],
      ['DELETE FROM bifrost_remote_invoke_calls WHERE started_at < ?', [cutoff]],
      ['DELETE FROM bifrost_remote_invoke_pairings WHERE create_time < ?', [cutoff]],
      ['DELETE FROM bifrost_remote_invoke_ssh_claims WHERE expires_at < ?', [_now]],
    ] as const) {
      const [result] = await this.pool.execute<ResultSetHeader>(sql, params as unknown as ExecuteValues[]);
      total += result.affectedRows;
    }

    const [countRows] = await this.pool.execute<RowDataPacket[]>('SELECT COUNT(*) as cnt FROM bifrost_remote_invoke_calls');
    const count = countRows[0]?.cnt as number ?? 0;
    if (count > maxRecords) {
      const excess = count - maxRecords;
      const [result] = await this.pool.execute<ResultSetHeader>(
        'DELETE FROM bifrost_remote_invoke_calls WHERE id IN (SELECT id FROM (SELECT id FROM bifrost_remote_invoke_calls ORDER BY started_at ASC LIMIT ?) AS old_calls)',
        [excess],
      );
      total += result.affectedRows;
    }

    return total;
  }
}

export class MysqlStorage implements IStorage {
  public user: MysqlUserDao;
  public env: MysqlEnvDao;
  public basicConfig: MysqlBasicConfigDao;
  public group: MysqlGroupDao;
  public groupMember: MysqlGroupMemberDao;
  public groupSetting: MysqlGroupSettingDao;
  public remoteInvoke: MysqlRemoteInvokeDao;
  private pool: Pool;

  constructor(config: MysqlConfig) {
    this.pool = mysql.createPool({
      host: config.host,
      port: config.port,
      user: config.user,
      password: config.password,
      database: config.database,
      waitForConnections: true,
      connectionLimit: 10,
    });
    this.user = new MysqlUserDao(this.pool);
    this.env = new MysqlEnvDao(this.pool);
    this.basicConfig = new MysqlBasicConfigDao(this.pool);
    this.group = new MysqlGroupDao(this.pool);
    this.groupMember = new MysqlGroupMemberDao(this.pool);
    this.groupSetting = new MysqlGroupSettingDao(this.pool);
    this.remoteInvoke = new MysqlRemoteInvokeDao(this.pool);
  }

  async ready(): Promise<void> {
    await this.basicConfig.ready();
    await this.remoteInvoke.ready();
  }

  async close(): Promise<void> {
    await this.ready().catch(() => undefined);
    await this.pool.end();
  }
}
