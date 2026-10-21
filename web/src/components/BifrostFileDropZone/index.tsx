import { useCallback, useEffect, useState } from "react";
import { Modal, Spin, message } from "antd";
import { useNavigate } from "react-router-dom";
import { UploadOutlined } from "@ant-design/icons";
import {
  formatBifrostFileError,
  formatImportSuccessMessage,
  getImportedItemCount,
  importFile,
  previewFile,
} from "../../api/bifrost-file";
import type { BifrostFileType } from "../../api/bifrost-file";
import { confirmBifrostFileImport } from "../BifrostFilePreview";
import { useValuesStore } from "../../stores/useValuesStore";
import { useScriptsStore } from "../../stores/useScriptsStore";
import { useReplayStore } from "../../stores/useReplayStore";
import { useRulesStore } from "../../stores/useRulesStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import { t as translate, useI18n } from "../../i18n";
import "./style.css";

interface DropZoneProps {
  children: React.ReactNode;
  onImportSuccess?: (fileType: BifrostFileType) => void;
}

export const fileTypeRoutes: Record<BifrostFileType, string> = {
  rules: "/rules",
  network: "/traffic",
  script: "/scripts",
  values: "/values",
  template: "/replay",
};

export const refreshStoreByType = async (fileType: BifrostFileType) => {
  switch (fileType) {
    case "values":
      await useValuesStore.getState().fetchValues();
      break;
    case "script":
      await useScriptsStore.getState().fetchScripts();
      break;
    case "template":
      await useReplayStore.getState().loadGroups();
      await useReplayStore.getState().loadSavedRequests();
      break;
    case "rules":
      await useRulesStore.getState().fetchRules();
      break;
    case "network":
      {
        const trafficStore = useTrafficStore.getState();
        trafficStore.setToolbarFilters({
          ...trafficStore.toolbarFilters,
          imported: ["Imported"],
        });
      }
      break;
    default:
      break;
  }
};

export const importBifrostFileContent = async (
  content: string,
  filename: string,
  navigate: (route: string) => void,
  onImportSuccess?: (fileType: BifrostFileType) => void,
  setImporting?: (importing: boolean) => void,
) => {
  const preview = await previewFile(content);
  const confirmed = await confirmBifrostFileImport(filename, preview);
  if (!confirmed) {
    return;
  }

  setImporting?.(true);
  try {
    const result = await importFile(content);
    const importedCount = getImportedItemCount(result);

    if (importedCount === 0) {
      message.warning(formatImportSuccessMessage(result, filename));
      return;
    }

    if (result.warnings && result.warnings.length > 0) {
      message.warning(
        translate("app.import.importedWithWarnings", {
          filename,
          count: result.warnings.length,
        }),
      );
    } else {
      message.success(formatImportSuccessMessage(result, filename));
    }

    await refreshStoreByType(result.file_type);
    onImportSuccess?.(result.file_type);

    const route = fileTypeRoutes[result.file_type];
    if (route) {
      navigate(route);
    }
  } finally {
    setImporting?.(false);
  }
};

export const BifrostFileDropZone: React.FC<DropZoneProps> = ({
  children,
  onImportSuccess,
}) => {
  const [isDragging, setIsDragging] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const navigate = useNavigate();
  const { t } = useI18n();

  const handleDragOver = useCallback((e: DragEvent) => {
    if (!e.dataTransfer?.types.includes("Files")) return;

    e.preventDefault();
    e.stopPropagation();
    setIsDragging(true);
  }, []);

  const handleDragLeave = useCallback((e: DragEvent) => {
    if (!e.dataTransfer?.types.includes("Files")) return;

    e.preventDefault();
    e.stopPropagation();

    if (
      e.relatedTarget === null ||
      !(e.relatedTarget instanceof Node) ||
      !document.body.contains(e.relatedTarget)
    ) {
      setIsDragging(false);
    }
  }, []);

  const handleDrop = useCallback(
    async (e: DragEvent) => {
      if (!e.dataTransfer?.types.includes("Files")) return;

      e.preventDefault();
      e.stopPropagation();
      setIsDragging(false);

      const files = Array.from(e.dataTransfer?.files || []);
      const bifrostFiles = files.filter((f) => f.name.endsWith(".bifrost"));

      if (bifrostFiles.length === 0) {
        message.warning(t("app.import.dropBifrostFile"));
        return;
      }

      try {
        for (const file of bifrostFiles) {
          const content = await file.text();
          await importBifrostFileContent(
            content,
            file.name,
            navigate,
            onImportSuccess,
            setIsImporting,
          );
        }
      } catch (error) {
        message.error(
          t("app.import.failed", { error: formatBifrostFileError(error) }),
        );
      }
    },
    [navigate, onImportSuccess, t],
  );

  useEffect(() => {
    window.addEventListener("dragover", handleDragOver);
    window.addEventListener("dragleave", handleDragLeave);
    window.addEventListener("drop", handleDrop);

    return () => {
      window.removeEventListener("dragover", handleDragOver);
      window.removeEventListener("dragleave", handleDragLeave);
      window.removeEventListener("drop", handleDrop);
    };
  }, [handleDragOver, handleDragLeave, handleDrop]);

  return (
    <>
      {children}

      {isDragging && (
        <div className="bifrost-drop-overlay">
          <div className="bifrost-drop-content">
            <UploadOutlined style={{ fontSize: 48 }} />
            <span>{t("app.import.dropToImport")}</span>
          </div>
        </div>
      )}

      <Modal
        open={isImporting}
        footer={null}
        closable={false}
        centered
        width={200}
      >
        <div style={{ textAlign: "center", padding: 20 }}>
          <Spin size="large" />
          <p style={{ marginTop: 16, marginBottom: 0 }}>
            {t("app.import.importing")}
          </p>
        </div>
      </Modal>
    </>
  );
};

export default BifrostFileDropZone;
