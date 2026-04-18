import crypto from 'crypto';
import mysql, { type Pool, type RowDataPacket, type ResultSetHeader } from 'mysql2/promise';
import { nanoid } from 'nanoid';
import type {
  Env, User, CreateEnvReq, UpdateEnvReq, SearchEnvQuery, MysqlConfig,
  Group, GroupMember, GroupSetting, UpdateGroupReq, SearchGroupQuery, UpdateGroupSettingReq,
} from '../types';
import type { IUserDao, IEnvDao, IStorage, IGroupDao, IGroupMemberDao, IGroupSettingDao, IRemoteInvokeDao } from './types';

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

export class MysqlStorage implements IStorage {
  public user: MysqlUserDao;
  public env: MysqlEnvDao;
  public group: MysqlGroupDao;
  public groupMember: MysqlGroupMemberDao;
  public groupSetting: MysqlGroupSettingDao;
  public remoteInvoke: IRemoteInvokeDao;
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
    this.group = new MysqlGroupDao(this.pool);
    this.groupMember = new MysqlGroupMemberDao(this.pool);
    this.groupSetting = new MysqlGroupSettingDao(this.pool);
    this.remoteInvoke = new Proxy({} as IRemoteInvokeDao, {
      get: () => () => { throw new Error('remote invoke not supported for MySQL storage'); },
    });
  }

  async close(): Promise<void> {
    await this.pool.end();
  }
}
