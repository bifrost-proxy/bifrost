import { Modal, Button, Typography, theme, message, Divider, Progress } from "antd";
import {
  RocketOutlined,
  CopyOutlined,
  ExportOutlined,
  CheckCircleOutlined,
  InfoCircleOutlined,
  LoadingOutlined,
} from "@ant-design/icons";
import type { CSSProperties } from "react";
import { useMemo, useCallback } from "react";
import { copyToClipboard } from "../../utils/clipboard";
import { useVersionStore } from "../../stores/useVersionStore";
import { useMetricsStore } from "../../stores/useMetricsStore";
import { isDesktopShell } from "../../runtime";
import { useI18n } from "../../i18n";

const { Text, Link } = Typography;

export default function VersionModal() {
  const { token } = theme.useToken();
  const { t } = useI18n();
  const modalVisible = useVersionStore((state) => state.modalVisible);
  const setModalVisible = useVersionStore((state) => state.setModalVisible);
  const hasUpdate = useVersionStore((state) => state.hasUpdate);
  const currentVersion = useVersionStore((state) => state.currentVersion);
  const latestVersion = useVersionStore((state) => state.latestVersion);
  const releaseHighlights = useVersionStore((state) => state.releaseHighlights);
  const releaseUrl = useVersionStore((state) => state.releaseUrl);
  const overview = useMetricsStore((state) => state.overview);
  const desktopMode = isDesktopShell();
  const upgradeCommand = desktopMode ? "bifrost app upgrade" : "bifrost upgrade";

  // Upgrade flow state.
  const upgrading = useVersionStore((state) => state.upgrading);
  const upgradePhase = useVersionStore((state) => state.upgradePhase);
  const upgradePercent = useVersionStore((state) => state.upgradePercent);
  const upgradeMessage = useVersionStore((state) => state.upgradeMessage);
  const upgradeError = useVersionStore((state) => state.upgradeError);
  const markVersionSeen = useVersionStore((state) => state.markVersionSeen);
  const startUpgrade = useVersionStore((state) => state.startUpgrade);

  const isFailed = upgradePhase === "failed";
  // The modal is locked (no manual close, no "Remind later") while the upgrade
  // is mid-flight; on failure we re-open the controls so the user can retry.
  const upgradeInProgress = upgrading && !isFailed;

  const handleClose = useCallback(() => {
    setModalVisible(false);
  }, [setModalVisible]);

  const handleRemindLater = useCallback(() => {
    if (latestVersion) {
      markVersionSeen(latestVersion);
    }
    setModalVisible(false);
  }, [latestVersion, markVersionSeen, setModalVisible]);

  const handleUpdateNow = useCallback(() => {
    void startUpgrade();
  }, [startUpgrade]);

  const handleCopyCommand = useCallback(async () => {
    const ok = await copyToClipboard(upgradeCommand);
    if (ok) {
      message.success(t("app.version.commandCopied"));
    } else {
      message.error(t("app.version.copyCommandFailed"));
    }
  }, [upgradeCommand, t]);

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
          <p style={styles.headerTitle}>{t("app.version.newVersionAvailable")}</p>
          <p style={styles.headerSubtitle}>
            {desktopMode
              ? t("app.version.desktopUpdateReady")
              : t("app.version.newerVersionReady")}
          </p>
        </div>
      </div>

      <div style={styles.versionRow}>
        <div style={{ textAlign: "center" }}>
          <div style={styles.versionLabel}>{t("app.version.current")}</div>
          <div style={{ ...styles.versionValue, ...styles.versionCurrent }}>
            v{currentVersion}
          </div>
        </div>
        <span style={styles.arrow}>→</span>
        <div style={{ textAlign: "center" }}>
          <div style={styles.versionLabel}>{t("app.version.latest")}</div>
          <div style={{ ...styles.versionValue, ...styles.versionLatest }}>
            v{latestVersion}
          </div>
        </div>
      </div>

      {releaseHighlights.length > 0 && (
        <div style={styles.section}>
          <div style={styles.sectionTitle}>
            <span>✨</span>
            <span>{t("app.version.whatsNew")}</span>
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
          <span>{t("app.version.upgradeCommand")}</span>
        </div>
        <div style={styles.commandBox}>
          <Text style={styles.command} copyable={false}>
            {upgradeCommand}
          </Text>
          <Button
            type="text"
            size="small"
            icon={<CopyOutlined />}
            onClick={handleCopyCommand}
            data-testid="version-upgrade-copy-button"
          >
            {t("common.copy")}
          </Button>
        </div>
      </div>

      {releaseUrl && (
        <div style={styles.releaseLink}>
          <Link href={releaseUrl} target="_blank">
            <ExportOutlined /> {t("app.version.viewFullReleaseNotes")}
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
            <p style={styles.headerTitle}>{t("app.version.versionInformation")}</p>
            <p style={styles.headerSubtitle}>{t("app.version.installationDetails")}</p>
          </div>
        </div>

        <div style={styles.versionRow}>
          <div style={{ textAlign: "center" }}>
            <div style={styles.versionLabel}>{t("app.version.version")}</div>
            <div style={{ ...styles.versionValue, ...styles.versionCurrent }}>
              v{currentVersion || systemInfo?.version}
            </div>
          </div>
        </div>

        {systemInfo && (
          <div style={styles.section}>
            <div style={styles.infoRow}>
              <div style={styles.infoItem}>
                <span style={styles.infoLabel}>{t("app.version.os")}</span>
                <span style={styles.infoValue}>
                  {systemInfo.os} ({systemInfo.arch})
                </span>
              </div>
            </div>
          </div>
        )}

        <div style={styles.successBadge}>
          <CheckCircleOutlined />
          <span>{t("app.version.runningLatest")}</span>
        </div>

        {releaseHighlights.length > 0 && (
          <>
            <Divider style={{ margin: "16px 0" }} />
            <div style={styles.section}>
              <div style={styles.sectionTitle}>
                <span>✨</span>
                <span>{t("app.version.releaseHighlights")}</span>
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
            <ExportOutlined /> {t("app.version.viewReleaseHistory")}
          </Link>
        </div>
      </div>
    );
  };

  const renderUpgradeProgress = () => {
    const downloading = upgradePhase === "downloading";
    const percent =
      downloading && typeof upgradePercent === "number"
        ? Math.round(upgradePercent)
        : undefined;
    const installing =
      upgradePhase === "installing" || upgradePhase === "restarting";

    return (
      <div style={styles.modalContent} data-testid="version-upgrade-progress">
        <div style={styles.header}>
          {isFailed ? (
            <InfoCircleOutlined
              style={{ ...styles.headerIcon, color: token.colorError }}
            />
          ) : (
            <LoadingOutlined style={styles.headerIcon} />
          )}
          <div style={styles.headerText}>
            <p style={styles.headerTitle}>
              {isFailed ? t("app.version.updateFailed") : t("app.version.updating")}
            </p>
            <p style={styles.headerSubtitle}>
              {isFailed
                ? t("app.version.previousStillAvailable")
                : desktopMode
                  ? t("app.version.installingDesktop", { version: String(latestVersion) })
                  : t("app.version.installing", { version: String(latestVersion) })}
            </p>
          </div>
        </div>

        {!isFailed && (
          <div style={styles.section}>
            <div style={styles.sectionTitle}>
              <span>⬇️</span>
              <span>{t("common.download")}</span>
            </div>
            <Progress
              percent={downloading ? (percent ?? 0) : upgradePhase === "checking" ? 0 : 100}
              status={
                installing || upgradePhase === "completed" ? "success" : "active"
              }
              data-testid="version-download-progress"
            />
          </div>
        )}

        {!isFailed && (
          <div style={styles.section}>
            <div style={styles.sectionTitle}>
              <span>📦</span>
              <span>{t("app.version.install")}</span>
            </div>
            <Progress
              percent={
                upgradePhase === "completed"
                  ? 100
                  : installing
                    ? 99
                    : 0
              }
              status={upgradePhase === "completed" ? "success" : "active"}
              data-testid="version-install-progress"
            />
          </div>
        )}

        <div
          style={{
            ...styles.headerSubtitle,
            marginTop: 8,
            color: isFailed ? token.colorError : token.colorTextSecondary,
          }}
        >
          {isFailed
            ? upgradeError || t("app.version.upgradeFailedRetry")
            : upgradeMessage || t("app.version.working")}
        </div>
      </div>
    );
  };

  const renderFooter = () => {
    if (!hasUpdate) {
      return (
        <Button type="primary" onClick={handleClose}>
          {t("app.version.gotIt")}
        </Button>
      );
    }
    if (upgradeInProgress) {
      // Locked: no actionable buttons while the upgrade runs.
      return null;
    }
    if (isFailed) {
      return (
        <>
          <Button onClick={handleClose}>{t("common.close")}</Button>
          <Button
            type="primary"
            onClick={handleUpdateNow}
            data-testid="version-retry-button"
          >
            {t("common.retry")}
          </Button>
        </>
      );
    }
    return (
      <>
        <Button
          onClick={handleRemindLater}
          data-testid="version-remind-later-button"
        >
          {t("app.version.remindLater")}
        </Button>
        <Button
          type="primary"
          onClick={handleUpdateNow}
          data-testid="version-update-now-button"
        >
          {t("app.version.updateNow")}
        </Button>
      </>
    );
  };

  const showProgressView = hasUpdate && (upgrading || isFailed);

  return (
    <Modal
      open={modalVisible}
      onCancel={handleClose}
      closable={!upgradeInProgress}
      maskClosable={!upgradeInProgress}
      keyboard={!upgradeInProgress}
      footer={renderFooter()}
      width={480}
      centered
      destroyOnClose
    >
      {showProgressView
        ? renderUpgradeProgress()
        : hasUpdate
          ? renderUpdateContent()
          : renderCurrentVersionContent()}
    </Modal>
  );
}
