import { create } from 'zustand';
import type { Group, GroupMember, GroupUserLevel } from '../api/group';
import * as groupApi from '../api/group';
import { isConnectionIssueError, normalizeApiErrorMessage } from '../api/client';
import { t } from '../i18n';

interface GroupState {
  groups: Group[];
  total: number;
  currentGroup: Group | null;
  myLevel: GroupUserLevel | null;
  members: GroupMember[];
  membersTotal: number;
  membersPage: number;
  membersKeyword: string;
  loading: boolean;
  membersLoading: boolean;
  error: string | null;

  fetchGroups: (keyword?: string) => Promise<void>;
  fetchGroupDetail: (id: string, userId?: string) => Promise<void>;
  createGroup: (req: groupApi.CreateGroupReq) => Promise<Group | null>;
  updateGroup: (id: string, req: groupApi.UpdateGroupReq) => Promise<true | string>;
  deleteGroup: (id: string) => Promise<boolean>;
  fetchMembers: (groupId: string, keyword?: string, offset?: number, limit?: number) => Promise<void>;
  setMembersPage: (page: number) => void;
  setMembersKeyword: (keyword: string) => void;
  inviteMembers: (groupId: string, req: groupApi.InviteGroupReq) => Promise<boolean>;
  removeMember: (groupId: string, userId: string) => Promise<boolean>;
  updateMemberLevel: (groupId: string, userId: string, level: groupApi.GroupUserLevel) => Promise<boolean>;
  leaveGroup: (id: string) => Promise<boolean>;
  clearError: () => void;
  clearCurrentGroup: () => void;
}

const MEMBERS_PAGE_SIZE = 20;

export const useGroupStore = create<GroupState>((set, get) => ({
  groups: [],
  total: 0,
  currentGroup: null,
  myLevel: null,
  members: [],
  membersTotal: 0,
  membersPage: 1,
  membersKeyword: '',
  loading: false,
  membersLoading: false,
  error: null,

  fetchGroups: async (keyword?: string) => {
    set({ loading: true, error: null });
    try {
      const result = await groupApi.searchGroups(keyword);
      set({ groups: result.list ?? [], total: result.total ?? 0, loading: false });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
    }
  },

  fetchGroupDetail: async (id: string, userId?: string) => {
    set({ loading: true, error: null });
    try {
      const promises: [Promise<groupApi.Group>, Promise<groupApi.RoomListResponse | null>] = [
        groupApi.getGroup(id),
        userId
          ? groupApi.searchRoom({ group_id: [id], user_id: [userId] })
          : Promise.resolve(null),
      ];
      const [group, roomResult] = await Promise.all(promises);

      const cachedGroup = get().groups.find(g => g.id === id);

      if (group.visibility === 'private' && cachedGroup?.visibility === 'public') {
        group.visibility = cachedGroup.visibility;
      }

      const room = roomResult?.list?.[0];
      const level: GroupUserLevel | null = room
        ? (room.level as GroupUserLevel)
        : (cachedGroup?.level ?? null);

      set({ currentGroup: group, myLevel: level, loading: false });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
    }
  },

  createGroup: async (req) => {
    set({ loading: true, error: null });
    try {
      const group = await groupApi.createGroup(req);
      await get().fetchGroups();
      return group;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return null;
    }
  },

  updateGroup: async (id, req) => {
    set({ loading: true, error: null });
    try {
      await groupApi.updateGroup(id, req);
      const userId = undefined;
      await get().fetchGroupDetail(id, userId);
      await get().fetchGroups();
      set({ loading: false });
      return true;
    } catch (e) {
      const msg = isConnectionIssueError(e) ? t('shared.connectionError') : normalizeApiErrorMessage(e);
      set({ error: isConnectionIssueError(e) ? null : msg, loading: false });
      return msg;
    }
  },

  deleteGroup: async (id) => {
    set({ loading: true, error: null });
    try {
      await groupApi.deleteGroup(id);
      set({ currentGroup: null, myLevel: null });
      await get().fetchGroups();
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return false;
    }
  },

  fetchMembers: async (groupId, keyword, offset, limit) => {
    set({ membersLoading: true, error: null });
    try {
      const result = await groupApi.getGroupMembers(
        groupId,
        keyword || undefined,
        offset ?? 0,
        limit ?? MEMBERS_PAGE_SIZE,
      );
      set({ members: result.list ?? [], membersTotal: result.total ?? 0, membersLoading: false });
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), membersLoading: false });
    }
  },

  setMembersPage: (page: number) => set({ membersPage: page }),
  setMembersKeyword: (keyword: string) => set({ membersKeyword: keyword, membersPage: 1 }),

  inviteMembers: async (groupId, req) => {
    set({ loading: true, error: null });
    try {
      await groupApi.inviteMembers(groupId, req);
      const { membersKeyword, membersPage } = get();
      await get().fetchMembers(groupId, membersKeyword, (membersPage - 1) * MEMBERS_PAGE_SIZE);
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return false;
    }
  },

  removeMember: async (groupId, userId) => {
    set({ loading: true, error: null });
    try {
      await groupApi.removeMember(groupId, userId);
      const { membersKeyword, membersPage } = get();
      await get().fetchMembers(groupId, membersKeyword, (membersPage - 1) * MEMBERS_PAGE_SIZE);
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return false;
    }
  },

  updateMemberLevel: async (groupId, userId, level) => {
    set({ loading: true, error: null });
    try {
      await groupApi.updateMemberLevel(groupId, userId, level);
      const { membersKeyword, membersPage } = get();
      await get().fetchMembers(groupId, membersKeyword, (membersPage - 1) * MEMBERS_PAGE_SIZE);
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return false;
    }
  },

  leaveGroup: async (id) => {
    set({ loading: true, error: null });
    try {
      await groupApi.leaveGroup(id);
      set({ currentGroup: null, myLevel: null });
      await get().fetchGroups();
      return true;
    } catch (e) {
      set({ error: isConnectionIssueError(e) ? null : normalizeApiErrorMessage(e), loading: false });
      return false;
    }
  },

  clearError: () => set({ error: null }),
  clearCurrentGroup: () => set({
    currentGroup: null,
    myLevel: null,
    members: [],
    membersTotal: 0,
    membersPage: 1,
    membersKeyword: '',
  }),
}));
