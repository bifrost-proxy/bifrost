import { get, post, del } from './client';
import client from './client';
import type { RuleSyntaxReport } from '../types';

export type GroupVisibility = 'public' | 'private';

export type GroupUserLevel = 0 | 1 | 2;

export interface Group {
  id: string;
  name: string;
  avatar: string;
  description: string;
  visibility: GroupVisibility;
  level?: GroupUserLevel | null;
  created_by?: string;
  create_time: string;
  update_time: string;
}

interface RemoteGroup {
  id: string;
  name: string;
  avatar: string | null;
  what: string;
  visibility: number | null;
  level?: number | null;
  created_by?: string;
  create_time: string;
  update_time: string;
}

interface RemoteResponse<T> {
  code: number;
  message: string;
  data: T;
}

interface RemoteListPayload<T> {
  list: T[];
  total: number;
}

function normalizeVisibility(v: number | null | undefined): GroupVisibility {
  return v === 1 ? 'public' : 'private';
}

function normalizeGroup(raw: RemoteGroup): Group {
  return {
    id: raw.id,
    name: raw.name,
    avatar: raw.avatar || '',
    description: raw.what || '',
    visibility: normalizeVisibility(raw.visibility),
    level: (raw.level === 0 || raw.level === 1 || raw.level === 2) ? raw.level as GroupUserLevel : null,
    created_by: raw.created_by,
    create_time: raw.create_time,
    update_time: raw.update_time,
  };
}

function unwrap<T>(resp: RemoteResponse<T>): T {
  if (resp.code !== 0) {
    throw new Error(resp.message || 'Request failed');
  }
  return resp.data;
}

export interface Room {
  id: string;
  group_id: string;
  user_id: string;
  level: GroupUserLevel;
  create_time: string;
  update_time: string;
}

export interface RoomListResponse {
  list: Room[];
  total: number;
}

export interface GroupMember {
  id: string;
  group_id: string;
  user_id: string;
  level: GroupUserLevel;
  nickname: string;
  avatar: string;
  email: string;
  create_time: string;
  update_time: string;
}

export interface GroupSetting {
  group_id: string;
  rules_enabled: boolean;
  visibility: GroupVisibility;
}

export interface GroupListResponse {
  list: Group[];
  total: number;
}

export interface GroupMemberListResponse {
  list: GroupMember[];
  total: number;
}

export interface CreateGroupReq {
  name: string;
  avatar?: string;
  description?: string;
  visibility?: GroupVisibility;
}

export interface UpdateGroupReq {
  name?: string;
  avatar?: string;
  description?: string;
  visibility?: GroupVisibility;
}

export interface InviteGroupReq {
  user_ids: string[];
  level?: GroupUserLevel;
}

export interface UpdateGroupSettingReq {
  rules_enabled?: boolean;
  visibility?: GroupVisibility;
}

function mapSettingReqToRemote(req: UpdateGroupSettingReq): Record<string, unknown> {
  const body: Record<string, unknown> = {};
  if (req.rules_enabled !== undefined) {
    body.status = req.rules_enabled;
  }
  if (req.visibility !== undefined) {
    body.level = req.visibility === 'public' ? 1 : 0;
  }
  return body;
}

async function patch<T>(url: string, data?: unknown): Promise<T> {
  const response = await client.patch<T>(url, data);
  return response.data;
}

export async function searchGroups(keyword?: string, offset = 0, limit = 50): Promise<GroupListResponse> {
  const params = new URLSearchParams();
  if (keyword) params.set('keyword', keyword);
  params.set('offset', String(offset));
  params.set('limit', String(limit));
  const resp = await get<RemoteResponse<RemoteListPayload<RemoteGroup>>>(`/group?${params.toString()}`);
  const payload = unwrap(resp);
  return {
    list: (payload.list ?? []).map(normalizeGroup),
    total: payload.total ?? 0,
  };
}

export async function getGroup(id: string): Promise<Group> {
  const resp = await get<RemoteResponse<RemoteGroup>>(`/group/${id}`);
  return normalizeGroup(unwrap(resp));
}

export async function createGroup(req: CreateGroupReq): Promise<Group> {
  const { description, visibility, ...rest } = req;
  const body: Record<string, unknown> = { ...rest };
  if (description !== undefined) {
    body.what = description;
  }
  if (visibility !== undefined) {
    body.visibility = visibility;
  }
  const resp = await post<RemoteResponse<RemoteGroup>>('/group', body);
  const group = normalizeGroup(unwrap(resp));
  if (visibility === 'public') {
    await updateGroupSetting(group.id, { rules_enabled: true, visibility: 'public' });
  }
  return group;
}

export async function updateGroup(id: string, req: UpdateGroupReq): Promise<void> {
  const { description, ...rest } = req;
  const body: Record<string, unknown> = { ...rest };
  if (description !== undefined) {
    body.what = description;
  }
  const resp = await patch<RemoteResponse<unknown>>(`/group/${id}`, body);
  if (resp.code !== 0) {
    throw new Error(resp.message || 'Failed to update group');
  }
}

export async function deleteGroup(id: string): Promise<void> {
  await del(`/group/${id}`);
}

export async function getGroupMembers(id: string, keyword?: string, offset = 0, limit = 50): Promise<GroupMemberListResponse> {
  const params = new URLSearchParams();
  params.set('keyword', keyword || '');
  params.set('offset', String(offset));
  params.set('limit', String(limit));
  const resp = await get<RemoteResponse<RemoteListPayload<GroupMember>>>(`/group/${id}/members?${params.toString()}`);
  const payload = unwrap(resp);
  return {
    list: payload.list ?? [],
    total: payload.total ?? 0,
  };
}

export async function inviteMembers(groupId: string, req: InviteGroupReq): Promise<void> {
  await Promise.all(
    req.user_ids.map((userId) =>
      post('/room', { group_id: groupId, user_id: userId, level: req.level ?? 0 }),
    ),
  );
}

export async function searchRoom(params: { group_id?: string[]; user_id?: string[]; keyword?: string; offset?: number; limit?: number }): Promise<RoomListResponse> {
  const qs = new URLSearchParams();
  if (params.group_id) params.group_id.forEach(id => qs.append('group_id', id));
  if (params.user_id) params.user_id.forEach(id => qs.append('user_id', id));
  if (params.keyword) qs.set('keyword', params.keyword);
  if (params.offset != null) qs.set('offset', String(params.offset));
  if (params.limit != null) qs.set('limit', String(params.limit));
  const resp = await get<RemoteResponse<RemoteListPayload<Room>>>(`/room?${qs.toString()}`);
  const payload = unwrap(resp);
  return {
    list: payload.list ?? [],
    total: payload.total ?? 0,
  };
}

export async function removeMember(groupId: string, userId: string): Promise<void> {
  const qs = new URLSearchParams();
  qs.set('group_id', groupId);
  qs.set('user_id', userId);
  await del(`/room?${qs.toString()}`);
}

export async function updateMemberLevel(groupId: string, userId: string, level: GroupUserLevel): Promise<void> {
  await patch('/room', { group_id: groupId, user_id: userId, level });
}

export async function leaveGroup(id: string): Promise<void> {
  await post(`/group/${id}/leave`);
}

export interface UserInfo {
  id: string;
  user_id: string;
  nickname?: string;
  avatar?: string;
  email?: string;
  channel?: number;
  create_time: string;
  update_time: string;
}

export interface UserListResponse {
  list: UserInfo[];
  total: number;
}

export async function searchUsers(keyword: string, offset = 0, limit = 10): Promise<UserListResponse> {
  const params = new URLSearchParams();
  if (keyword) params.set('keyword', keyword);
  params.set('offset', String(offset));
  params.set('limit', String(limit));
  const resp = await get<RemoteResponse<RemoteListPayload<UserInfo>>>(`/user?${params.toString()}`);
  const payload = unwrap(resp);
  return {
    list: payload.list ?? [],
    total: payload.total ?? 0,
  };
}

interface RemoteGroupSetting {
  status: boolean;
  level: number;
}

export async function getGroupSetting(id: string): Promise<GroupSetting> {
  const resp = await get<RemoteResponse<RemoteGroupSetting>>(`/group/${id}/setting`);
  const raw = unwrap(resp);
  return {
    group_id: id,
    rules_enabled: raw.status,
    visibility: raw.level === 1 ? 'public' : 'private',
  };
}

export async function updateGroupSetting(id: string, req: UpdateGroupSettingReq): Promise<void> {
  await patch(`/group/${id}/setting`, mapSettingReqToRemote(req));
}

export interface RemoteEnv {
  id: string;
  user_id: string;
  name: string;
  rule: string;
  create_time: string;
  update_time: string;
}

export interface EnvListResponse {
  list: RemoteEnv[];
  total: number;
}

export async function searchEnvs(userIds: string[], offset = 0, limit = 500): Promise<EnvListResponse> {
  const results = await Promise.allSettled(
    userIds.map(async (userId) => {
      try {
        const qs = new URLSearchParams();
        qs.append('user_id', userId);
        qs.set('offset', String(offset));
        qs.set('limit', String(limit));
        const resp = await get<RemoteResponse<RemoteListPayload<RemoteEnv>>>(`/env?${qs.toString()}`);
        const payload = unwrap(resp);
        return payload.list ?? [];
      } catch {
        return [] as RemoteEnv[];
      }
    })
  );
  const allEnvs: RemoteEnv[] = [];
  for (const result of results) {
    if (result.status === 'fulfilled') {
      allEnvs.push(...result.value);
    }
  }
  return {
    list: allEnvs,
    total: allEnvs.length,
  };
}

export interface GroupRuleInfo {
  name: string;
  enabled: boolean;
  sort_order: number;
  rule_count: number;
  created_at: string;
  updated_at: string;
  remote_env_id?: string;
  remote_user_id?: string;
}

export interface GroupRulesResponse {
  group_id: string;
  group_name: string;
  writable: boolean;
  rules: GroupRuleInfo[];
}

export interface GroupRuleDetail {
  name: string;
  content: string;
  enabled: boolean;
  sort_order: number;
  created_at: string;
  updated_at: string;
  sync: {
    status: 'local_only' | 'synced' | 'modified';
    remote_id?: string;
    remote_updated_at?: string;
  };
  syntax?: RuleSyntaxReport;
}

export async function fetchGroupRules(groupId: string): Promise<GroupRulesResponse> {
  const resp = await get<GroupRulesResponse>(`/group-rules/${groupId}`);
  return resp;
}

export async function getGroupRule(groupId: string, ruleName: string): Promise<GroupRuleDetail> {
  const encoded = encodeURIComponent(ruleName);
  const resp = await get<GroupRuleDetail>(`/group-rules/${groupId}/${encoded}`);
  return resp;
}

export async function createGroupRule(groupId: string, name: string, content?: string): Promise<GroupRuleDetail> {
  const resp = await post<GroupRuleDetail>(`/group-rules/${groupId}`, { name, content: content ?? '' });
  return resp;
}

export async function updateGroupRule(groupId: string, ruleName: string, content: string): Promise<GroupRuleDetail> {
  const encoded = encodeURIComponent(ruleName);
  const resp = await client.put<GroupRuleDetail>(`/group-rules/${groupId}/${encoded}`, { content });
  return resp.data;
}

export async function deleteGroupRule(groupId: string, ruleName: string): Promise<void> {
  const encoded = encodeURIComponent(ruleName);
  await del(`/group-rules/${groupId}/${encoded}`);
}

export async function enableGroupRule(groupId: string, ruleName: string): Promise<void> {
  const encoded = encodeURIComponent(ruleName);
  await client.put(`/group-rules/${groupId}/${encoded}/enable`);
}

export async function disableGroupRule(groupId: string, ruleName: string): Promise<void> {
  const encoded = encodeURIComponent(ruleName);
  await client.put(`/group-rules/${groupId}/${encoded}/disable`);
}
