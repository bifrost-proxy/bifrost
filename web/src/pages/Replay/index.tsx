import { useEffect, type CSSProperties } from "react";
import { Spin, Tooltip, theme } from "antd";
import { EditOutlined, HistoryOutlined } from "@ant-design/icons";
import { useReplayStore, type ReplayMode } from "../../stores/useReplayStore";
import SplitPane from "../../components/SplitPane";
import VerticalSplitPane from "../../components/VerticalSplitPane";
import CollectionPanel from "./components/CollectionPanel";
import RequestPanel from "./components/RequestPanel";
import ResponsePanel from "./components/ResponsePanel";
import HistoryView from "./components/HistoryView";
import pushService from "../../services/pushService";
import { isMacDesktopShell } from "../../runtime";

interface ModeButtonProps {
  mode: ReplayMode;
  icon: React.ReactNode;
  label: string;
  isActive: boolean;
  onClick: () => void;
}

const ModeButton = ({ mode, icon, label, isActive, onClick }: ModeButtonProps) => {
  const { token } = theme.useToken();

  return (
    <Tooltip title={label} placement="left">
      <div
        onClick={onClick}
        data-testid={`replay-mode-${mode}`}
        style={{
          width: 32,
          height: 32,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          cursor: "pointer",
          borderRadius: 4,
          backgroundColor: isActive ? token.colorPrimaryBg : "transparent",
          color: isActive ? token.colorPrimary : token.colorTextSecondary,
          transition: "all 0.2s",
        }}
        onMouseEnter={(e) => {
          if (!isActive) {
            e.currentTarget.style.backgroundColor = token.colorBgTextHover;
          }
        }}
        onMouseLeave={(e) => {
          if (!isActive) {
            e.currentTarget.style.backgroundColor = "transparent";
          }
        }}
      >
        {icon}
      </div>
    </Tooltip>
  );
};

export default function Replay() {
  const { token } = theme.useToken();
  const {
    currentRequest,
    savedRequests,
    loading,
    uiState,
    loadGroups,
    loadSavedRequests,
    loadAllHistory,
    updateUIState,
    selectRequest,
  } = useReplayStore();

  const currentMode = uiState.mode;

  useEffect(() => {
    const init = async () => {
      await Promise.all([
        loadGroups(),
        loadSavedRequests(),
      ]);

      pushService.connect({
        need_replay_saved_requests: true,
        need_replay_groups: true,
      });
    };
    init();

    return () => {
      pushService.updateSubscription({
        need_replay_saved_requests: false,
        need_replay_groups: false,
      });
      pushService.disconnectIfIdle();
    };
  }, [loadGroups, loadSavedRequests]);

  useEffect(() => {
    if (
      !uiState.selectedRequestId ||
      savedRequests.length === 0 ||
      currentRequest?.id === uiState.selectedRequestId
    ) {
      return;
    }
    const savedRequest = savedRequests.find((item) => item.id === uiState.selectedRequestId);
    if (savedRequest) {
      void selectRequest(savedRequest);
    }
  }, [currentRequest?.id, savedRequests, selectRequest, uiState.selectedRequestId]);

  const handleModeChange = (mode: ReplayMode) => {
    if (mode === "history") {
      void loadAllHistory(1);
    }
    updateUIState({ mode });
  };

  const styles: Record<string, CSSProperties> = {
    container: {
      height: "100%",
      width: "100%",
      overflow: "hidden",
      backgroundColor: isMacDesktopShell() ? "transparent" : token.colorBgContainer,
      display: "flex",
    },
    mainContent: {
      flex: 1,
      height: "100%",
      overflow: "hidden",
    },
    toolbar: {
      width: 40,
      height: "100%",
      borderLeft: `1px solid ${token.colorBorderSecondary}`,
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      paddingTop: 8,
      gap: 4,
      backgroundColor: isMacDesktopShell() ? "transparent" : token.colorBgLayout,
    },
    collectionPanel: {
      height: "100%",
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
    },
    centerArea: {
      height: "100%",
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
      minHeight: 0,
    },
    requestPanel: {
      flex: 1,
      minHeight: 0,
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
    },
    responsePanel: {
      flex: 1,
      minHeight: 0,
      overflow: "hidden",
      display: "flex",
      flexDirection: "column",
    },
    historyView: {
      height: "100%",
      overflow: "hidden",
    },
  };

  const showSpinner = loading;

  const composerContent = (
    <div style={styles.centerArea}>
      <Spin spinning={showSpinner} style={{ height: "100%", display: "flex", flexDirection: "column", minHeight: 0 }}>
        <VerticalSplitPane
          defaultTopHeight="55%"
          minTopHeight={200}
          minBottomHeight={150}
          top={
            <div style={styles.requestPanel}>
              <RequestPanel />
            </div>
          }
          bottom={
            <div style={styles.responsePanel}>
              <ResponsePanel />
            </div>
          }
        />
      </Spin>
    </div>
  );

  const historyContent = (
    <div style={styles.historyView}>
      <HistoryView />
    </div>
  );

  return (
    <div style={styles.container}>
      <div style={styles.mainContent}>
        <SplitPane
          defaultLeftWidth="280px"
          minLeftWidth={200}
          minRightWidth={500}
          transparentLeftOnMacDesktop
          left={
            <div style={styles.collectionPanel}>
              <CollectionPanel />
            </div>
          }
          right={currentMode === "composer" ? composerContent : historyContent}
        />
      </div>
      <div style={styles.toolbar}>
        <ModeButton
          mode="composer"
          icon={<EditOutlined style={{ fontSize: 16 }} />}
          label="Composer"
          isActive={currentMode === "composer"}
          onClick={() => handleModeChange("composer")}
        />
        <ModeButton
          mode="history"
          icon={<HistoryOutlined style={{ fontSize: 16 }} />}
          label="History"
          isActive={currentMode === "history"}
          onClick={() => handleModeChange("history")}
        />
      </div>
    </div>
  );
}
