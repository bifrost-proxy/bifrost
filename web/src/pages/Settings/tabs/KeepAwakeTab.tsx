import { useCallback, useEffect, useState } from "react";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Radio,
  Space,
  Switch,
  Tag,
  Typography,
  message,
} from "antd";
import {
  PoweroffOutlined,
  ReloadOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import {
  getPowerStatus,
  setPowerMode,
  setPowerOff,
  setPowerOn,
  type KeepAwakeMode,
  type KeepAwakeStatus,
} from "../../../api/power";

const { Text, Paragraph } = Typography;

const MODE_LABELS: Record<KeepAwakeMode, string> = {
  off: "Off",
  auto: "Auto (recommended)",
  force_on: "Force On",
};

const MODE_DESCRIPTIONS: Record<KeepAwakeMode, string> = {
  off: "Never prevent sleep. The Mac can sleep on idle or when the lid is closed.",
  auto: "Automatically keep the Mac awake whenever a remote command is received. Stays awake until you manually disable it or Bifrost exits.",
  force_on: "Always keep the Mac awake while Bifrost is running. Equivalent to pressing the global switch.",
};

export default function KeepAwakeTab() {
  const [status, setStatus] = useState<KeepAwakeStatus | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [busy, setBusy] = useState<boolean>(false);

  const refresh = useCallback(async () => {
    try {
      setLoading(true);
      const s = await getPowerStatus();
      setStatus(s);
    } catch (err) {
      message.error(`Failed to load keep-awake status: ${(err as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => {
      void refresh();
    }, 10_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const handleSwitch = useCallback(
    async (checked: boolean) => {
      if (!status?.supported) return;
      setBusy(true);
      try {
        const next = checked ? await setPowerOn() : await setPowerOff();
        setStatus(next);
        message.success(checked ? "Keep-awake enabled" : "Keep-awake disabled");
      } catch (err) {
        message.error(`Failed: ${(err as Error).message}`);
      } finally {
        setBusy(false);
      }
    },
    [status?.supported],
  );

  const handleModeChange = useCallback(
    async (mode: KeepAwakeMode) => {
      setBusy(true);
      try {
        const next = await setPowerMode(mode);
        setStatus(next);
        message.success(`Mode set to ${MODE_LABELS[mode]}`);
      } catch (err) {
        message.error(`Failed: ${(err as Error).message}`);
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  if (!status && loading) {
    return <Card loading />;
  }

  const supported = status?.supported ?? false;

  return (
    <Space direction="vertical" style={{ width: "100%" }} size="large">
      {!supported && (
        <Alert
          type="warning"
          showIcon
          message="Keep-awake is only supported on macOS"
          description={`Current platform: ${status?.platform ?? "unknown"}. Switching modes has no effect.`}
        />
      )}

      {status?.battery_warning && (
        <Alert type="info" showIcon message={status.battery_warning} />
      )}

      <Card
        title={
          <Space>
            <ThunderboltOutlined />
            <span>Keep Awake</span>
          </Space>
        }
        extra={
          <Button
            icon={<ReloadOutlined />}
            size="small"
            onClick={() => void refresh()}
            loading={loading}
          >
            Refresh
          </Button>
        }
      >
        <Paragraph>
          <Text>
            Prevents display sleep, idle sleep, and lid-close sleep while Bifrost is
            running. Uses macOS IOKit (no <code>sudo</code> required). Releases the
            assertion automatically when Bifrost exits.
          </Text>
        </Paragraph>

        <Descriptions column={1} size="small" bordered>
          <Descriptions.Item label="Currently Active">
            <Space>
              {status?.active ? (
                <Tag color="success" icon={<PoweroffOutlined />}>ON</Tag>
              ) : (
                <Tag color="default">OFF</Tag>
              )}
              {status?.active && status.active_since_secs != null && (
                <Text type="secondary">
                  for {formatDuration(status.active_since_secs)}
                </Text>
              )}
            </Space>
          </Descriptions.Item>
          <Descriptions.Item label="Power Source">
            <Tag color={status?.on_battery ? "orange" : "blue"}>
              {status?.on_battery ? "Battery" : "AC"}
            </Tag>
          </Descriptions.Item>
          <Descriptions.Item label="Global Switch">
            <Switch
              checked={status?.active ?? false}
              onChange={handleSwitch}
              loading={busy}
              disabled={!supported}
            />
          </Descriptions.Item>
        </Descriptions>

        <div style={{ marginTop: 24 }}>
          <Text strong>Mode</Text>
          <Paragraph type="secondary" style={{ marginBottom: 8 }}>
            Controls when Bifrost activates keep-awake. The current mode is persisted
            to <code>config.toml</code>.
          </Paragraph>
          <Radio.Group
            value={status?.mode}
            onChange={(e) => void handleModeChange(e.target.value as KeepAwakeMode)}
            disabled={!supported || busy}
          >
            <Space direction="vertical">
              {(["off", "auto", "force_on"] as KeepAwakeMode[]).map((m) => (
                <Radio key={m} value={m}>
                  <Text strong>{MODE_LABELS[m]}</Text>
                  <br />
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {MODE_DESCRIPTIONS[m]}
                  </Text>
                </Radio>
              ))}
            </Space>
          </Radio.Group>
        </div>
      </Card>
    </Space>
  );
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m ${secs % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}
