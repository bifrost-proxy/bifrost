import { useCallback, useEffect, useState } from "react";
import { Modal, Spin, message } from "antd";
import { useNavigate } from "react-router-dom";
import { UploadOutlined } from "@ant-design/icons";
import {
  formatBifrostFileError,
  formatImportSuccessMessage,
  getImportedItemCount,
  importFile,
} from "../../api/bifrost-file";
import type { BifrostFileType } from "../../api/bifrost-file";
import { useValuesStore } from "../../stores/useValuesStore";
import { useScriptsStore } from "../../stores/useScriptsStore";
import { useReplayStore } from "../../stores/useReplayStore";
import { useRulesStore } from "../../stores/useRulesStore";
import { useTrafficStore } from "../../stores/useTrafficStore";
import "./style.css";

interface DropZoneProps {
  children: React.ReactNode;
  onImportSuccess?: (fileType: BifrostFileType) => void;
}

const fileTypeRoutes: Record<BifrostFileType, string> = {
  rules: "/rules",
  network: "/traffic",
  script: "/scripts",
  values: "/values",
  template: "/replay",
};

const refreshStoreByType = async (fileType: BifrostFileType) => {
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

export const BifrostFileDropZone: React.FC<DropZoneProps> = ({
  children,
  onImportSuccess,
}) => {
  const [isDragging, setIsDragging] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const navigate = useNavigate();

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
        message.warning("Please drop a .bifrost file");
        return;
      }

      setIsImporting(true);

      try {
        for (const file of bifrostFiles) {
          const content = await file.text();
          const result = await importFile(content);
          const importedCount = getImportedItemCount(result);

          if (importedCount === 0) {
            message.warning(formatImportSuccessMessage(result, file.name));
            continue;
          } else if (result.warnings && result.warnings.length > 0) {
            message.warning(
              `Imported ${file.name} with ${result.warnings.length} warning(s)`,
            );
          } else {
            message.success(formatImportSuccessMessage(result, file.name));
          }

          await refreshStoreByType(result.file_type);
          onImportSuccess?.(result.file_type);

          const route = fileTypeRoutes[result.file_type];
          if (route) {
            navigate(route);
          }
        }
      } catch (error) {
        message.error(`Import failed: ${formatBifrostFileError(error)}`);
      } finally {
        setIsImporting(false);
      }
    },
    [navigate, onImportSuccess],
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
            <span>Drop to import .bifrost file</span>
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
          <p style={{ marginTop: 16, marginBottom: 0 }}>Importing...</p>
        </div>
      </Modal>
    </>
  );
};

export default BifrostFileDropZone;
