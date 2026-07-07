export type { IUserDao, IEnvDao, IBasicConfigDao, IGroupDao, IGroupMemberDao, IGroupSettingDao, IRemoteInvokeDao, IStorage } from './types';
export { SqliteStorage } from './sqlite';
export { MysqlStorage } from './mysql';

import type { StorageConfig } from '../types';
import type { IStorage } from './types';
import { SqliteStorage } from './sqlite';
import { MysqlStorage } from './mysql';

export function createStorage(config: StorageConfig): IStorage {
  if (config.type === 'mysql') {
    if (!config.mysql) {
      throw new Error('storage.type is "mysql" but storage.mysql is not configured');
    }
    console.log(`[bifrost-sync-server] using MySQL storage (${config.mysql.host}:${config.mysql.port}/${config.mysql.database})`);
    return new MysqlStorage(config.mysql);
  }
  const dataDir = config.sqlite?.data_dir ?? './bifrost-sync-data';
  console.log(`[bifrost-sync-server] using SQLite storage (${dataDir})`);
  return new SqliteStorage(dataDir);
}
