import { Button, Modal, Typography, message } from "antd";
import { useCallback, useEffect, useState } from "react";
import { getNativeAppStatus, installNativeApp } from "../../api/nativeApp";
import type { NativeAppStatus } from "../../types";

const LATER_KEY = "bifrost-native-app-install-later";

export default function NativeAppInstallPrompt() {
  const [status, setStatus] = useState<NativeAppStatus | null>(null);
  const [visible, setVisible] = useState(false);
  const [installing, setInstalling] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const next = await getNativeAppStatus();
      setStatus(next);
      const laterVersion = localStorage.getItem(LATER_KEY);
      setVisible(
        next.supported &&
          next.needs_install &&
          laterVersion !== (next.latest_version || "current"),
      );
    } catch {
      setVisible(false);
    }
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void refresh();
    }, 1200);
    return () => window.clearTimeout(timer);
  }, [refresh]);

  const handleLater = useCallback(() => {
    localStorage.setItem(LATER_KEY, status?.latest_version || "current");
    setVisible(false);
  }, [status]);

  const handleInstall = useCallback(async () => {
    setInstalling(true);
    try {
      await installNativeApp();
      message.success("Native app installation started");
      window.setTimeout(() => {
        void refresh();
      }, 2500);
    } catch (error) {
      message.error(
        error instanceof Error ? error.message : "Failed to start native app installation",
      );
    } finally {
      setInstalling(false);
    }
  }, [refresh]);

  return (
    <Modal
      open={visible}
      title="Install Bifrost Native App"
      onCancel={handleLater}
      centered
      width={460}
      footer={[
        <Button key="later" onClick={handleLater}>
          Later
        </Button>,
        <Button
          key="install"
          type="primary"
          loading={installing}
          onClick={handleInstall}
          data-testid="native-app-install-button"
        >
          Install
        </Button>,
      ]}
    >
      <Typography.Paragraph>
        The macOS native app depends on the installed Bifrost CLI. Install it to{" "}
        <Typography.Text code>{status?.install_path || "/Applications/Bifrost.app"}</Typography.Text>{" "}
        and open it after installation.
      </Typography.Paragraph>
      {status?.download_url ? (
        <Typography.Paragraph type="secondary">
          Version {status.latest_version} will be downloaded from the Bifrost release assets.
        </Typography.Paragraph>
      ) : null}
    </Modal>
  );
}
