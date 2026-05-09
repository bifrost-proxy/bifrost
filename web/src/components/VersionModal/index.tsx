import { Modal, Button, Typography, theme, message, Divider } from "antd";
import {
  RocketOutlined,
  CopyOutlined,
  ExportOutlined,
  CheckCircleOutlined,
  InfoCircleOutlined,
} from "@ant-design/icons";
import type { CSSProperties } from "react";
import { useMemo, useCallback } from "react";
import { copyToClipboard } from "../../utils/clipboard";
import { useVersionStore } from "../../stores/useVersionStore";
import { useMetricsStore } from "../../stores/useMetricsStore";

const { Text, Link } = Typography;

export default function VersionModal() {
  const { token } = theme.useToken();
  const modalVisible = useVersionStore((state) => state.modalVisible);
  const setModalVisible = useVersionStore((state) => state.setModalVisible);
  const hasUpdate = useVersionStore((state) => state.hasUpdate);
  const currentVersion = useVersionStore((state) => state.currentVersion);
  const latestVersion = useVersionStore((state) => state.latestVersion);
  const releaseHighlights = useVersionStore((state) => state.releaseHighlights);
  const releaseUrl = useVersionStore((state) => state.releaseUrl);
  const overview = useMetricsStore((state) => state.overview);

  const handleClose = useCallback(() => {
    setModalVisible(false);
  }, [setModalVisible]);

  const handleCopyCommand = useCallback(async () => {
    const ok = await copyToClipboard("bifrost upgrade");
    if (ok) {
      message.success("Command copied to clipboard");
    } else {
      message.error("Failed to copy command");
    }
  }, []);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      modalContent: {
        padding: "8px 0",
      },
      header: {
        display: "flex",
        alignItems: "center",
        gap: 12,
        marginBottom: 20,
      },
      headerIcon: {
        fontSize: 32,
        color: hasUpdate ? token.colorPrimary : token.colorTextSecondary,
      },
      headerText: {
        flex: 1,
      },
      headerTitle: {
        fontSize: 18,
        fontWeight: 600,
        margin: 0,
        color: token.colorText,
      },
      headerSubtitle: {
        fontSize: 13,
        color: token.colorTextSecondary,
        margin: 0,
      },
      versionRow: {
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 16,
        padding: "16px 0",
        backgroundColor: token.colorFillQuaternary,
        borderRadius: 8,
        marginBottom: 16,
      },
      versionLabel: {
        fontSize: 12,
        color: token.colorTextSecondary,
        marginBottom: 4,
      },
      versionValue: {
        fontSize: 16,
        fontWeight: 600,
        fontFamily: "monospace",
      },
      versionCurrent: {
        color: hasUpdate ? token.colorTextSecondary : token.colorSuccess,
      },
      versionLatest: {
        color: token.colorSuccess,
      },
      arrow: {
        fontSize: 18,
        color: token.colorTextSecondary,
      },
      section: {
        marginBottom: 16,
      },
      sectionTitle: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        fontSize: 14,
        fontWeight: 600,
        color: token.colorText,
        marginBottom: 12,
      },
      highlightList: {
        margin: 0,
        paddingLeft: 20,
      },
      highlightItem: {
        fontSize: 13,
        color: token.colorTextSecondary,
        lineHeight: 1.8,
      },
      commandBox: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "10px 12px",
        backgroundColor: token.colorFillQuaternary,
        borderRadius: 6,
        border: `1px solid ${token.colorBorderSecondary}`,
      },
      command: {
        flex: 1,
        fontFamily: "monospace",
        fontSize: 13,
        color: token.colorText,
      },
      infoRow: {
        display: "flex",
        gap: 24,
        flexWrap: "wrap" as const,
      },
      infoItem: {
        display: "flex",
        gap: 8,
      },
      infoLabel: {
        fontSize: 13,
        color: token.colorTextSecondary,
      },
      infoValue: {
        fontSize: 13,
        color: token.colorText,
        fontFamily: "monospace",
      },
      successBadge: {
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "12px 16px",
        backgroundColor: token.colorSuccessBg,
        borderRadius: 6,
        color: token.colorSuccess,
        fontSize: 13,
      },
      releaseLink: {
        display: "flex",
        alignItems: "center",
        gap: 6,
        marginTop: 8,
      },
    }),
    [token, hasUpdate],
  );

  const renderUpdateContent = () => (
    <div style={styles.modalContent}>
      <div style={styles.header}>
        <RocketOutlined style={styles.headerIcon} />
        <div style={styles.headerText}>
          <p style={styles.headerTitle}>New Version Available</p>
          <p style={styles.headerSubtitle}>
            A newer version of Bifrost is ready to install
          </p>
        </div>
      </div>

      <div style={styles.versionRow}>
        <div style={{ textAlign: "center" }}>
          <div style={styles.versionLabel}>Current</div>
          <div style={{ ...styles.versionValue, ...styles.versionCurrent }}>
            v{currentVersion}
          </div>
        </div>
        <span style={styles.arrow}>→</span>
        <div style={{ textAlign: "center" }}>
          <div style={styles.versionLabel}>Latest</div>
          <div style={{ ...styles.versionValue, ...styles.versionLatest }}>
            v{latestVersion}
          </div>
        </div>
      </div>

      {releaseHighlights.length > 0 && (
        <div style={styles.section}>
          <div style={styles.sectionTitle}>
            <span>✨</span>
            <span>What's New</span>
          </div>
          <ul style={styles.highlightList}>
            {releaseHighlights.map((highlight, index) => (
              <li key={index} style={styles.highlightItem}>
                {highlight}
              </li>
            ))}
          </ul>
        </div>
      )}

      <Divider style={{ margin: "16px 0" }} />

      <div style={styles.section}>
        <div style={styles.sectionTitle}>
          <span>📦</span>
          <span>Upgrade Command</span>
        </div>
        <div style={styles.commandBox}>
          <Text style={styles.command} copyable={false}>
            bifrost upgrade
          </Text>
          <Button
            type="text"
            size="small"
            icon={<CopyOutlined />}
            onClick={handleCopyCommand}
            data-testid="version-upgrade-copy-button"
          >
            Copy
          </Button>
        </div>
      </div>

      {releaseUrl && (
        <div style={styles.releaseLink}>
          <Link href={releaseUrl} target="_blank">
            <ExportOutlined /> View Full Release Notes
          </Link>
        </div>
      )}
    </div>
  );

  const renderCurrentVersionContent = () => {
    const systemInfo = overview?.system;

    return (
      <div style={styles.modalContent}>
        <div style={styles.header}>
          <InfoCircleOutlined style={styles.headerIcon} />
          <div style={styles.headerText}>
            <p style={styles.headerTitle}>Version Information</p>
            <p style={styles.headerSubtitle}>Current installation details</p>
          </div>
        </div>

        <div style={styles.versionRow}>
          <div style={{ textAlign: "center" }}>
            <div style={styles.versionLabel}>Version</div>
            <div style={{ ...styles.versionValue, ...styles.versionCurrent }}>
              v{currentVersion || systemInfo?.version}
            </div>
          </div>
        </div>

        {systemInfo && (
          <div style={styles.section}>
            <div style={styles.infoRow}>
              <div style={styles.infoItem}>
                <span style={styles.infoLabel}>OS:</span>
                <span style={styles.infoValue}>
                  {systemInfo.os} ({systemInfo.arch})
                </span>
              </div>
            </div>
          </div>
        )}

        <div style={styles.successBadge}>
          <CheckCircleOutlined />
          <span>You are running the latest version</span>
        </div>

        {releaseHighlights.length > 0 && (
          <>
            <Divider style={{ margin: "16px 0" }} />
            <div style={styles.section}>
              <div style={styles.sectionTitle}>
                <span>✨</span>
                <span>Release Highlights</span>
              </div>
              <ul style={styles.highlightList}>
                {releaseHighlights.map((highlight, index) => (
                  <li key={index} style={styles.highlightItem}>
                    {highlight}
                  </li>
                ))}
              </ul>
            </div>
          </>
        )}

        <div style={styles.releaseLink}>
          <Link
            href={
              releaseUrl || "https://github.com/bifrost-proxy/bifrost/releases"
            }
            target="_blank"
          >
            <ExportOutlined /> View Release History
          </Link>
        </div>
      </div>
    );
  };

  return (
    <Modal
      open={modalVisible}
      onCancel={handleClose}
      footer={
        <Button type="primary" onClick={handleClose}>
          Got it
        </Button>
      }
      width={480}
      centered
      destroyOnClose
    >
      {hasUpdate ? renderUpdateContent() : renderCurrentVersionContent()}
    </Modal>
  );
}
