import { useEffect, useState, useCallback, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import {
  Input,
  Tag,
  Empty,
  Spin,
  Avatar,
  Row,
  Col,
  Typography,
  Space,
  Modal,
  Form,
  Select,
  message,
  Card,
  theme,
  Alert,
} from "antd";
import {
  PlusOutlined,
  LockOutlined,
  GlobalOutlined,
  TeamOutlined,
} from "@ant-design/icons";
import { useGroupStore } from "../../stores/useGroupStore";
import type { Group, GroupVisibility } from "../../api/group";
import { isMacDesktopShell } from "../../runtime";
import { useI18n, type TranslationKey } from "../../i18n";

const { Title, Paragraph } = Typography;

const randomColor = () =>
  "#" +
  Math.floor(Math.random() * 16777215)
    .toString(16)
    .padStart(6, "0");

const roleLabelMap: Record<number, { textKey: TranslationKey; color: string }> = {
  2: { textKey: "groups.role.owner", color: "gold" },
  1: { textKey: "groups.role.master", color: "blue" },
  0: { textKey: "groups.role.member", color: "default" },
};

function GroupCardGrid({
  groups,
  onClickGroup,
  prependSlot,
}: {
  groups: Group[];
  onClickGroup: (id: string) => void;
  prependSlot?: React.ReactNode;
}) {
  const { token } = theme.useToken();
  const { t } = useI18n();

  return (
    <Row gutter={[16, 16]}>
      {prependSlot && <Col xs={24} sm={12} md={8} lg={6}>{prependSlot}</Col>}
      {groups.map((group) => {
        const roleInfo =
          group.level != null ? roleLabelMap[group.level] : null;
        return (
          <Col key={group.id} xs={24} sm={12} md={8} lg={6}>
            <Card
              hoverable
              onClick={() => onClickGroup(group.id)}
              style={{ height: "100%" }}
              styles={{ body: { padding: 16 } }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                <Avatar
                  size={48}
                  style={{
                    backgroundColor: group.avatar || token.colorPrimary,
                    fontSize: 20,
                    fontWeight: 600,
                    flexShrink: 0,
                  }}
                >
                  {group.name?.[0]?.toUpperCase()}
                </Avatar>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                    <Title
                      level={5}
                      style={{ margin: 0, flex: 1, minWidth: 0 }}
                      ellipsis={{ rows: 1 }}
                    >
                      {group.name}
                    </Title>
                    <Tag
                      color={group.visibility === "public" ? "green" : "default"}
                      style={{ marginRight: 0 }}
                    >
                      {group.visibility === "public" ? t("groups.public") : t("groups.private")}
                    </Tag>
                    {roleInfo && (
                      <Tag color={roleInfo.color} style={{ marginRight: 0 }}>
                        {t(roleInfo.textKey)}
                      </Tag>
                    )}
                  </div>
                  <Paragraph
                    type="secondary"
                    style={{ margin: 0, fontSize: 13 }}
                    ellipsis={{ rows: 1 }}
                  >
                    {group.description || t("groups.noDescription")}
                  </Paragraph>
                </div>
              </div>
            </Card>
          </Col>
        );
      })}
    </Row>
  );
}

export default function Groups() {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const navigate = useNavigate();
  const groups = useGroupStore((s) => s.groups);
  const loading = useGroupStore((s) => s.loading);
  const fetchGroups = useGroupStore((s) => s.fetchGroups);
  const createGroup = useGroupStore((s) => s.createGroup);

  const [searchKeyword, setSearchKeyword] = useState("");
  const [isSearching, setIsSearching] = useState(false);
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [createLoading, setCreateLoading] = useState(false);
  const [form] = Form.useForm();

  useEffect(() => {
    void fetchGroups();
  }, [fetchGroups]);

  const handleSearch = useCallback(
    (value: string) => {
      const trimmed = value.trim();
      setSearchKeyword(trimmed);
      setIsSearching(!!trimmed);
      void fetchGroups(trimmed || undefined);
    },
    [fetchGroups],
  );

  const managedGroups = useMemo(
    () => (groups ?? []).filter((g) => g.level != null && g.level >= 1),
    [groups],
  );

  const joinedGroups = useMemo(
    () => (groups ?? []).filter((g) => g.level === 0),
    [groups],
  );

  const discoverGroups = useMemo(
    () => (groups ?? []).filter((g) => g.level == null && g.visibility === "public"),
    [groups],
  );

  const handleCreateSubmit = useCallback(async () => {
    try {
      const values = await form.validateFields();
      setCreateLoading(true);
      const group = await createGroup({
        name: values.name,
        description: values.description,
        visibility: values.visibility,
        avatar: randomColor(),
      });
      if (group) {
        message.success(t("groups.list.created"));
        setCreateModalOpen(false);
        form.resetFields();
        navigate(`/groups/${group.id}`);
      } else {
        const err = useGroupStore.getState().error;
        (window as unknown as Record<string, unknown>).__BIFROST_LAST_ERROR__ = err;
        message.error(err || t("groups.list.createFailed"));
      }
    } finally {
      setCreateLoading(false);
    }
  }, [form, createGroup, navigate, t]);

  const handleCreateCancel = useCallback(() => {
    setCreateModalOpen(false);
    form.resetFields();
  }, [form]);

  const handleClickGroup = useCallback(
    (id: string) => navigate(`/groups/${id}`),
    [navigate],
  );

  const createCardSlot = (
    <Card
      hoverable
      onClick={() => setCreateModalOpen(true)}
      style={{
        height: "100%",
        borderStyle: "dashed",
      }}
      styles={{ body: { padding: 16 } }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          gap: 8,
          height: 48,
          color: token.colorTextSecondary,
        }}
      >
        <PlusOutlined style={{ fontSize: 18 }} />
        <span style={{ fontSize: 14, fontWeight: 500 }}>{t("groups.list.createGroup")}</span>
      </div>
    </Card>
  );

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
        <Alert
          type="info"
          showIcon
          icon={<TeamOutlined />}
          message={t("groups.list.intro")}
          style={{ marginBottom: 20 }}
        />
        <div
          style={{
            display: "flex",
            justifyContent: "center",
            marginBottom: 24,
          }}
        >
          <Input.Search
            placeholder={t("groups.list.searchPlaceholder")}
            allowClear
            value={searchKeyword}
            onChange={(e) => setSearchKeyword(e.target.value)}
            onSearch={handleSearch}
            style={{ width: 400, maxWidth: "100%" }}
            size="large"
          />
        </div>

        {loading && groups.length === 0 ? (
          <div
            style={{
              display: "flex",
              justifyContent: "center",
              padding: "80px 0",
            }}
          >
            <Spin size="large" />
          </div>
        ) : isSearching ? (
          groups.length === 0 ? (
            <Empty description={t("groups.list.noGroupsFound")} style={{ padding: "80px 0" }} />
          ) : (
            <GroupCardGrid
              groups={groups}
              onClickGroup={handleClickGroup}
            />
          )
        ) : (
          <>
            <div style={{ marginBottom: 24 }}>
              <Title level={5} style={{ marginBottom: 12 }}>
                {t("groups.list.managed")}
              </Title>
              <GroupCardGrid
                groups={managedGroups}
                onClickGroup={handleClickGroup}
                prependSlot={createCardSlot}
              />
            </div>
            {joinedGroups.length > 0 && (
              <div style={{ marginBottom: 24 }}>
                <Title level={5} style={{ marginBottom: 12 }}>
                  {t("groups.list.joined")}
                </Title>
                <GroupCardGrid
                  groups={joinedGroups}
                  onClickGroup={handleClickGroup}
                />
              </div>
            )}
            {discoverGroups.length > 0 && (
              <div style={{ marginBottom: 24 }}>
                <Title level={5} style={{ marginBottom: 12 }}>
                  {t("groups.list.discover")}
                </Title>
                <GroupCardGrid
                  groups={discoverGroups}
                  onClickGroup={handleClickGroup}
                />
              </div>
            )}
            {managedGroups.length === 0 && joinedGroups.length === 0 && discoverGroups.length === 0 && (
              <Empty
                description={t("groups.list.empty")}
                style={{ padding: "40px 0" }}
              />
            )}
          </>
        )}
      </div>

      <Modal
        title={t("groups.list.createGroup")}
        open={createModalOpen}
        onOk={handleCreateSubmit}
        onCancel={handleCreateCancel}
        confirmLoading={createLoading}
        destroyOnClose
      >
        <Form
          form={form}
          layout="vertical"
          initialValues={{ visibility: "private" as GroupVisibility }}
        >
          <Form.Item
            name="name"
            label={t("common.name")}
            rules={[{ required: true, message: t("groups.list.nameRequired") }]}
          >
            <Input placeholder={t("groups.list.namePlaceholder")} maxLength={50} />
          </Form.Item>
          <Form.Item name="description" label={t("common.description")}>
            <Input.TextArea
              placeholder={t("groups.list.descriptionPlaceholder")}
              rows={3}
              maxLength={200}
            />
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
    </div>
  );
}
