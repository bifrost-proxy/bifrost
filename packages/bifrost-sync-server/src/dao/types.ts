import type { Env, User, CreateEnvReq, UpdateEnvReq, SearchEnvQuery } from '../types';

export interface IUserDao {
  findByToken(token: string): Promise<User | undefined>;
  findByUserId(userId: string): Promise<User | undefined>;
  register(
    userId: string,
    password: string,
    fields: Partial<Pick<User, 'nickname' | 'avatar' | 'email'>>,
  ): Promise<User>;
  verifyPassword(userId: string, password: string): Promise<boolean>;
  saveToken(userId: string, token: string): Promise<void>;
  clearToken(userId: string): Promise<void>;
}

export interface IEnvDao {
  findById(id: string): Promise<Env | undefined>;
  findByUserAndName(userId: string, name: string): Promise<Env | undefined>;
  create(req: CreateEnvReq): Promise<Env>;
  update(id: string, fields: UpdateEnvReq): Promise<Env | undefined>;
  delete(id: string): Promise<boolean>;
  deleteByUserId(userId: string): Promise<number>;
  search(query: SearchEnvQuery): Promise<{ list: Env[]; total: number }>;
}

export interface IGroupDao {
  create(name: string, avatar: string, description: string, visibility: string, createdBy: string): Promise<import('../types').Group>;
  findById(id: string): Promise<import('../types').Group | undefined>;
  findByName(name: string): Promise<import('../types').Group | undefined>;
  update(id: string, fields: import('../types').UpdateGroupReq): Promise<import('../types').Group | undefined>;
  delete(id: string): Promise<boolean>;
  search(query: import('../types').SearchGroupQuery, userId?: string): Promise<{ list: import('../types').Group[]; total: number }>;
}

export interface IGroupMemberDao {
  add(groupId: string, userId: string, level: number): Promise<import('../types').GroupMember>;
  remove(groupId: string, userId: string): Promise<boolean>;
  updateLevel(groupId: string, userId: string, level: number): Promise<boolean>;
  findByGroupAndUser(groupId: string, userId: string): Promise<import('../types').GroupMember | undefined>;
  listByGroup(groupId: string, query?: { keyword?: string; offset?: number; limit?: number }): Promise<{ list: import('../types').GroupMember[]; total: number }>;
  listByUser(userId: string): Promise<import('../types').GroupMember[]>;
}

export interface IGroupSettingDao {
  get(groupId: string): Promise<import('../types').GroupSetting>;
  update(groupId: string, fields: import('../types').UpdateGroupSettingReq): Promise<void>;
  init(groupId: string, visibility?: string): Promise<void>;
}

export interface IRemoteInvokeDao {
  createPairing(pairing: import('../types').RemoteInvokePairing): Promise<import('../types').RemoteInvokePairing>;
  getPairing(pairingId: string): Promise<import('../types').RemoteInvokePairing | undefined>;
  updatePairing(pairingId: string, fields: Partial<import('../types').RemoteInvokePairing>): Promise<void>;
  findPairingByCode(userId: string, clientInstanceId: string, pairCode: string): Promise<import('../types').RemoteInvokePairing | undefined>;
  countPendingPairings(clientInstanceId: string): Promise<number>;
  listPendingPairings(clientInstanceId: string): Promise<import('../types').RemoteInvokePairing[]>;
  cancelPendingPairings(clientInstanceId: string): Promise<number>;

  createGrant(grant: import('../types').RemoteInvokeGrant): Promise<import('../types').RemoteInvokeGrant>;
  getGrant(grantId: string): Promise<import('../types').RemoteInvokeGrant | undefined>;
  findReusableGrant(userId: string, clientInstanceId: string, callerFingerprint: string): Promise<import('../types').RemoteInvokeGrant | undefined>;
  listGrants(userId: string, query: { client_instance_id?: string; status?: string; offset?: number; limit?: number }): Promise<{ list: import('../types').RemoteInvokeGrant[]; total: number }>;
  countActiveGrantsForClient(clientInstanceId: string): Promise<number>;
  listActiveGrantsForClient(clientInstanceId: string): Promise<import('../types').RemoteInvokeGrant[]>;
  updateGrant(grantId: string, fields: Partial<import('../types').RemoteInvokeGrant>): Promise<void>;
  deleteGrant(grantId: string): Promise<boolean>;
  touchGrantLastUsed(grantId: string, ts: string): Promise<void>;
  consumeGrantCall(grantId: string): Promise<void>;

  createCall(call: import('../types').RemoteInvokeCall): Promise<import('../types').RemoteInvokeCall>;
  getCall(callId: string): Promise<import('../types').RemoteInvokeCall | undefined>;
  updateCall(callId: string, fields: Partial<import('../types').RemoteInvokeCall>): Promise<void>;
  listCalls(userId: string, query: { client_instance_id?: string; caller_fingerprint?: string; status?: string; offset?: number; limit?: number }): Promise<{ list: import('../types').RemoteInvokeCall[]; total: number }>;

  appendEvent(event: import('../types').RemoteInvokeEvent): Promise<void>;
  listCallEvents(callId: string, query?: { offset?: number; limit?: number }): Promise<{ list: import('../types').RemoteInvokeEvent[]; total: number }>;

  registerClient(record: import('../types').RemoteInvokeClientRecord): Promise<void>;
  getClientRecord(clientInstanceId: string): Promise<import('../types').RemoteInvokeClientRecord | undefined>;
  updateClientRecord(clientInstanceId: string, fields: Partial<import('../types').RemoteInvokeClientRecord>): Promise<void>;

  cleanupExpiredData(now: string, retentionDays: number, maxRecords: number): Promise<number>;
}

export interface IStorage {
  user: IUserDao;
  env: IEnvDao;
  group: IGroupDao;
  groupMember: IGroupMemberDao;
  groupSetting: IGroupSettingDao;
  remoteInvoke: IRemoteInvokeDao;
  close(): Promise<void>;
}
