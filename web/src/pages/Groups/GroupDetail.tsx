import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import {
  Card,
  Button,
  Avatar,
  Tag,
  List,
  Typography,
  Space,
  Modal,
  Form,
  Input,
  Select,
  Popconfirm,
  Descriptions,
  message,
  Spin,
  Pagination,
  theme,
} from "antd";
import {
  ArrowLeftOutlined,
  EditOutlined,
  DeleteOutlined,
  UserAddOutlined,
  LogoutOutlined,
  UserDeleteOutlined,
  SearchOutlined,
  LockOutlined,
  GlobalOutlined,
} from "@ant-design/icons";
import dayjs from "dayjs";
import { useGroupStore } from "../../stores/useGroupStore";
import { useSyncStore } from "../../stores/useSyncStore";
import type { GroupUserLevel, GroupVisibility, UserInfo } from "../../api/group";
import { searchUsers, updateGroupSetting, updateGroup as apiUpdateGroup } from "../../api/group";
import { normalizeApiErrorMessage } from "../../api/client";
import { isMacDesktopShell } from "../../runtime";
import { useI18n, type TranslationKey } from "../../i18n";

const { Title, Text } = Typography;

const MEMBERS_PAGE_SIZE = 20;

const LEVEL_LABEL: Record<number, { textKey: TranslationKey; color: string }> = {
  2: { textKey: "groups.role.owner", color: "gold" },
  1: { textKey: "groups.role.master", color: "blue" },
  0: { textKey: "groups.role.member", color: "default" },
};

function formatTime(value?: string | null): string {
  if (!value) return "--";
  const date = dayjs(value);
  return date.isValid() ? date.format("YYYY-MM-DD HH:mm:ss") : value;
}

function getAvatarColor(name: string): string {
  const colors = [
    "#f56a00",
    "#7265e6",
    "#ffbf00",
    "#00a2ae",
    "#87d068",
    "#108ee9",
    "#f5222d",
    "#722ed1",
  ];
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  return colors[Math.abs(hash) % colors.length];
}

export default function GroupDetail() {
  const { id = "" } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const { token } = theme.useToken();
  const { t } = useI18n();

  const {
    currentGroup,
    myLevel,
    members,
    membersTotal,
    membersPage,
    membersKeyword,
    loading,
    membersLoading,
    fetchGroupDetail,
    fetchMembers,
    setMembersPage,
    setMembersKeyword,
    deleteGroup,
    inviteMembers,
    removeMember,
    updateMemberLevel,
    leaveGroup,
    clearCurrentGroup,
  } = useGroupStore();

  const syncStatus = useSyncStore((state) => state.syncStatus);
  const currentUserId = syncStatus?.user?.user_id;

  const [editModalOpen, setEditModalOpen] = useState(false);
  const [inviteModalOpen, setInviteModalOpen] = useState(false);
  const [editLoading, setEditLoading] = useState(false);
  const [inviteLoading, setInviteLoading] = useState(false);
  const [editForm] = Form.useForm();
  const [inviteForm] = Form.useForm();
  const [memberSearch, setMemberSearch] = useState("");

  const [userOptions, setUserOptions] = useState<UserInfo[]>([]);
  const [userSearchLoading, setUserSearchLoading] = useState(false);
  const searchTimerRef = useRef<ReturnType<typeof setTimeout>>(null);
  const searchStampRef = useRef(0);

  useEffect(() => {
    if (id) {
      fetchGroupDetail(id, currentUserId ?? undefined);
      fetchMembers(id);
    }
    return () => {
      clearCurrentGroup();
    };
  }, [id, currentUserId, fetchGroupDetail, fetchMembers, clearCurrentGroup]);

  useEffect(() => {
    if (id) {
      const offset = (membersPage - 1) * MEMBERS_PAGE_SIZE;
      fetchMembers(id, membersKeyword || undefined, offset, MEMBERS_PAGE_SIZE);
    }
  }, [id, membersPage, membersKeyword, fetchMembers]);

  const isMasterOrOwner = myLevel !== null && myLevel !== undefined && myLevel >= 1;
  const isOwner = myLevel === 2;

  const handleEdit = useCallback(() => {
    if (!currentGroup) return;
    editForm.setFieldsValue({
      name: currentGroup.name,
      description: currentGroup.description,
      visibility: currentGroup.visibility,
    });
    setEditModalOpen(true);
  }, [currentGroup, editForm]);

  const handleEditSubmit = useCallback(async () => {
    try {
      const values = await editForm.validateFields();
      const { visibility, ...groupFields } = values;
      setEditLoading(true);
      try {
        await apiUpdateGroup(id, { ...groupFields, visibility: visibility as GroupVisibility });
        if (visibility && visibility !== currentGroup?.visibility) {
          await updateGroupSetting(id, {
            rules_enabled: true,
            visibility: visibility as GroupVisibility,
          });
        }
        await fetchGroupDetail(id, currentUserId ?? undefined);
        message.success(t("groups.detail.updated"));
        setEditModalOpen(false);
      } catch (e) {
        message.error(normalizeApiErrorMessage(e) || t("groups.detail.updateFailed"));
      } finally {
        setEditLoading(false);
      }
    } catch {
      /* validation error */
    }
  }, [editForm, id, currentGroup, fetchGroupDetail, currentUserId, t]);

  const handleDelete = useCallback(async () => {
    const success = await deleteGroup(id);
    if (success) {
      message.success(t("groups.detail.deleted"));
      navigate("/groups");
    } else {
      const err = useGroupStore.getState().error;
      message.error(err || t("groups.detail.deleteFailed"));
    }
  }, [deleteGroup, id, navigate, t]);

  const handleInvite = useCallback(async () => {
    try {
      const values = await inviteForm.validateFields();
      const userIds: string[] = values.user_id ?? [];
      if (userIds.length === 0) return;
      setInviteLoading(true);
      const success = await inviteMembers(id, {
        user_ids: userIds,
        level: values.level,
      });
      if (success) {
        message.success(t("groups.detail.invitationSent"));
        setInviteModalOpen(false);
        inviteForm.resetFields();
        setUserOptions([]);
      } else {
        const err = useGroupStore.getState().error;
        message.error(err || t("groups.detail.invitationFailed"));
      }
      setInviteLoading(false);
    } catch {
      setInviteLoading(false);
    }
  }, [inviteForm, inviteMembers, id, t]);

  const handleRemoveMember = useCallback(
    async (userId: string) => {
      const success = await removeMember(id, userId);
      if (success) {
        message.success(t("groups.detail.memberRemoved"));
      } else {
        const err = useGroupStore.getState().error;
        message.error(err || t("groups.detail.removeMemberFailed"));
      }
    },
    [removeMember, id, t],
  );

  const handleUpdateLevel = useCallback(
    async (userId: string, level: GroupUserLevel) => {
      const success = await updateMemberLevel(id, userId, level);
      if (success) {
        message.success(t("groups.detail.levelUpdated"));
      } else {
        const err = useGroupStore.getState().error;
        message.error(err || t("groups.detail.updateLevelFailed"));
      }
    },
    [updateMemberLevel, id, t],
  );

  const handleLeave = useCallback(async () => {
    const success = await leaveGroup(id);
    if (success) {
      message.success(t("groups.detail.leftGroup"));
      navigate("/groups");
    } else {
      const err = useGroupStore.getState().error;
      message.error(err || t("groups.detail.leaveFailed"));
    }
  }, [leaveGroup, id, navigate, t]);

  const handleUserSearch = useCallback((value: string) => {
    if (searchTimerRef.current) {
      clearTimeout(searchTimerRef.current);
    }
    if (!value.trim()) {
      setUserOptions([]);
      return;
    }
    setUserSearchLoading(true);
    const stamp = Date.now();
    searchStampRef.current = stamp;
    searchTimerRef.current = setTimeout(async () => {
      try {
        const result = await searchUsers(value.trim());
        if (searchStampRef.current === stamp) {
          setUserOptions(result.list);
        }
      } catch {
        if (searchStampRef.current === stamp) {
          setUserOptions([]);
        }
      } finally {
        if (searchStampRef.current === stamp) {
          setUserSearchLoading(false);
        }
      }
    }, 500);
  }, []);

  const handleMemberSearch = useCallback(
    (value: string) => {
      setMemberSearch(value);
      setMembersKeyword(value.trim());
    },
    [setMembersKeyword],
  );

  const handlePageChange = useCallback(
    (page: number) => {
      setMembersPage(page);
    },
    [setMembersPage],
  );

  const sortedMembers = useMemo(() => {
    return [...members].sort((a, b) => {
      if (a.level !== b.level) return b.level - a.level;
      return (a.update_time ?? "").localeCompare(b.update_time ?? "");
    });
  }, [members]);

  if (loading && !currentGroup) {
    return (
      <div style={{ display: "flex", justifyContent: "center", alignItems: "center", height: "100%" }}>
        <Spin size="large" />
      </div>
    );
  }

  if (!currentGroup) {
    return null;
  }

  const avatarColor = getAvatarColor(currentGroup.name);

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        overflow: "auto",
        backgroundColor: isMacDesktopShell() ? "transparent" : token.colorBgLayout,
        padding: "0 24px 24px",
      }}
    >
      <div style={{ maxWidth: 1200, margin: "0 auto" }}>
        <Button
          type="text"
          icon={<ArrowLeftOutlined />}
          onClick={() => navigate("/groups")}
          style={{ marginBottom: 16 }}
        >
          {t("common.back")}
        </Button>

        <Card styles={{ body: { padding: 24 } }}>
          <div
            style={{
              display: "flex",
              alignItems: "flex-start",
              justifyContent: "space-between",
              gap: 24,
              flexWrap: "wrap",
            }}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 16, flex: "1 1 520px", minWidth: 0 }}>
              <Avatar
                size={64}
                style={{ backgroundColor: avatarColor, fontSize: 28, flexShrink: 0 }}
              >
                {currentGroup.name[0]?.toUpperCase()}
              </Avatar>
              <div style={{ flex: 1, minWidth: 0 }}>
                <Space align="center" size={8} wrap>
                  <Title level={4} style={{ margin: 0 }}>
                    {currentGroup.name}
                  </Title>
                  <Tag color={currentGroup.visibility === "public" ? "green" : "orange"}>
                    {currentGroup.visibility === "public" ? t("groups.public") : t("groups.private")}
                  </Tag>
                  {myLevel != null && (
                    <Tag color={LEVEL_LABEL[myLevel]?.color ?? "default"}>
                      {t(LEVEL_LABEL[myLevel]?.textKey ?? "groups.role.member")}
                    </Tag>
                  )}
                </Space>
                <div style={{ marginTop: 4 }}>
                  <Text type="secondary">
                    {currentGroup.description || t("groups.noDescription")}
                  </Text>
                </div>
              </div>
            </div>
            <Space wrap>
              {isMasterOrOwner && (
                <Button icon={<EditOutlined />} onClick={handleEdit}>
                  {t("common.edit")}
                </Button>
              )}
              {isOwner && (
                <Popconfirm
                  title={t("groups.detail.deleteGroup")}
                  description={t("groups.detail.deleteGroupConfirm")}
                  onConfirm={handleDelete}
                  okText={t("common.delete")}
                  okType="danger"
                >
                  <Button danger icon={<DeleteOutlined />}>
                    {t("common.delete")}
                  </Button>
                </Popconfirm>
              )}
            </Space>
          </div>

          <Descriptions column={{ xs: 1, sm: 2 }} style={{ marginTop: 20 }} size="small">
            <Descriptions.Item label={t("groups.detail.createdAt")}>
              {formatTime(currentGroup.create_time)}
            </Descriptions.Item>
            <Descriptions.Item label={t("groups.detail.updatedAt")}>
              {formatTime(currentGroup.update_time)}
            </Descriptions.Item>
          </Descriptions>
        </Card>

        <Card style={{ marginTop: 20 }} styles={{ body: { padding: 24 } }}>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              gap: 16,
              flexWrap: "wrap",
              marginBottom: 16,
            }}
          >
            <Title level={5} style={{ margin: 0 }}>
              {membersTotal
                ? t("groups.detail.membersWithCount", { count: membersTotal })
                : t("groups.detail.members")}
            </Title>
            <Space wrap>
              {isMasterOrOwner && (
                <Button
                  type="primary"
                  icon={<UserAddOutlined />}
                  onClick={() => {
                    inviteForm.resetFields();
                    setUserOptions([]);
                    setInviteModalOpen(true);
                  }}
                >
                  {t("groups.detail.addMembers")}
                </Button>
              )}
              {myLevel != null && !isOwner && (
                <Popconfirm
                  title={t("groups.detail.leaveGroup")}
                  description={t("groups.detail.leaveGroupConfirm")}
                  onConfirm={handleLeave}
                  okText={t("groups.detail.leave")}
                  okType="danger"
                >
                  <Button icon={<LogoutOutlined />}>{t("groups.detail.leave")}</Button>
                </Popconfirm>
              )}
            </Space>
          </div>

          <div style={{ marginBottom: 12 }}>
            <Input
              placeholder={t("groups.detail.searchMembers")}
              prefix={<SearchOutlined style={{ color: "#999" }} />}
              value={memberSearch}
              onChange={(e) => handleMemberSearch(e.target.value)}
              allowClear
              style={{ width: 360, maxWidth: "100%" }}
            />
          </div>

          <List
            dataSource={sortedMembers}
            loading={membersLoading}
            locale={{
              emptyText: membersKeyword
                ? t("groups.detail.noMatchingMembers")
                : t("groups.detail.noMembers"),
            }}
            renderItem={(member) => {
              const levelInfo = LEVEL_LABEL[member.level] || LEVEL_LABEL[0];
              const memberAvatarColor = getAvatarColor(member.nickname || member.user_id);
              const isAdmin = member.level > 0;
              const isSelf = member.user_id === currentUserId;
              return (
                <List.Item
                  actions={
                    isMasterOrOwner && !isSelf
                      ? [
                          <Button
                            key="toggle-admin"
                            type="text"
                            size="small"
                            onClick={() =>
                              handleUpdateLevel(
                                member.user_id,
                                isAdmin ? (0 as GroupUserLevel) : (1 as GroupUserLevel),
                              )
                            }
                          >
                            {isAdmin ? t("groups.detail.removeAdmin") : t("groups.detail.setAdmin")}
                          </Button>,
                          <Popconfirm
                            key="remove"
                            title={t("groups.detail.removeMember")}
                            description={t("groups.detail.removeMemberConfirm")}
                            onConfirm={() => handleRemoveMember(member.user_id)}
                            okText={t("common.remove")}
                            okType="danger"
                          >
                            <Button
                              type="text"
                              danger
                              size="small"
                              icon={<UserDeleteOutlined />}
                            >
                              {t("common.delete")}
                            </Button>
                          </Popconfirm>,
                        ]
                      : []
                  }
                >
                  <List.Item.Meta
                    avatar={
                      <Avatar
                        src={member.avatar || undefined}
                        style={{ backgroundColor: memberAvatarColor }}
                      >
                        {(member.nickname || member.user_id)[0]?.toUpperCase()}
                      </Avatar>
                    }
                    title={
                      <Space wrap>
                        <span>{member.nickname || member.user_id}</span>
                        {member.nickname && member.nickname !== member.user_id && (
                          <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                            {member.user_id}
                          </Typography.Text>
                        )}
                        {isAdmin && <Tag color={levelInfo.color}>{t(levelInfo.textKey)}</Tag>}
                        {isSelf && <Tag>{t("groups.detail.you")}</Tag>}
                      </Space>
                    }
                    description={t("groups.detail.joinedAt", { time: formatTime(member.create_time) })}
                  />
                </List.Item>
              );
            }}
          />
          {membersTotal > MEMBERS_PAGE_SIZE && (
            <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 12 }}>
              <Pagination
                current={membersPage}
                pageSize={MEMBERS_PAGE_SIZE}
                total={membersTotal}
                onChange={handlePageChange}
                showQuickJumper
                showSizeChanger={false}
                size="small"
              />
            </div>
          )}
        </Card>
      </div>

      <Modal
        title={t("groups.detail.editGroup")}
        open={editModalOpen}
        onOk={handleEditSubmit}
        onCancel={() => setEditModalOpen(false)}
        confirmLoading={editLoading}
      >
        <Form form={editForm} layout="vertical">
          <Form.Item
            name="name"
            label={t("common.name")}
            rules={[{ required: true, message: t("groups.detail.nameRequired") }]}
          >
            <Input />
          </Form.Item>
          <Form.Item name="description" label={t("common.description")}>
            <Input.TextArea rows={3} />
          </Form.Item>
          <Form.Item
            name="visibility"
            label={t("groups.visibility")}
            extra={
              <span style={{ fontSize: 12 }}>
                <b>{t("groups.public")}</b>{t("groups.visibilityPublicDesc")}<b>{t("groups.private")}</b>{t("groups.visibilityPrivateDesc")}
              </span>
            }
          >
            <Select>
              <Select.Option value="private">
                <Space>
                  <LockOutlined />
                  <span>{t("groups.private")}</span>
                </Space>
              </Select.Option>
              <Select.Option value="public">
                <Space>
                  <GlobalOutlined />
                  <span>{t("groups.public")}</span>
                </Space>
              </Select.Option>
            </Select>
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title={t("groups.detail.addMembers")}
        open={inviteModalOpen}
        onOk={handleInvite}
        onCancel={() => {
          setInviteModalOpen(false);
          setUserOptions([]);
        }}
        confirmLoading={inviteLoading}
      >
        <Form form={inviteForm} layout="vertical" initialValues={{ level: 0 }} autoComplete="off">
          <Form.Item
            name="user_id"
            label={t("groups.detail.user")}
            rules={[{ required: true, message: t("groups.detail.userRequired") }]}
          >
            <Select
              mode="multiple"
              showSearch
              filterOption={false}
              placeholder={t("groups.detail.userSearchPlaceholder")}
              loading={userSearchLoading}
              onSearch={handleUserSearch}
              onClear={() => setUserOptions([])}
              notFoundContent={userSearchLoading ? <Spin size="small" /> : null}
              options={userOptions.map((u) => ({
                label: (
                  <Space>
                    <Avatar
                      size={24}
                      src={u.avatar || undefined}
                      style={{ backgroundColor: getAvatarColor(u.nickname || u.user_id), flexShrink: 0 }}
                    >
                      {(u.nickname || u.user_id)[0]?.toUpperCase()}
                    </Avatar>
                    <span>{u.nickname || u.user_id}</span>
                    {u.nickname && u.nickname !== u.user_id && (
                      <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                        {u.user_id}
                      </Typography.Text>
                    )}
                  </Space>
                ),
                value: u.user_id,
              }))}
            />
          </Form.Item>
          <Form.Item name="level" label={t("groups.detail.role")}>
            <Select
              options={[
                { label: t("groups.role.member"), value: 0 },
                { label: t("groups.role.master"), value: 1 },
              ]}
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
