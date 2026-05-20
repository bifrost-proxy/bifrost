import { useCallback } from 'react';
import { message } from 'antd';
import {
  exportRules,
  exportNetwork,
  exportScripts,
  exportValues,
  exportTemplates,
  formatBifrostFileError,
  getEmptyExportMessage,
  getExportItemCount,
  downloadFile,
  formatExportFilename,
} from '../api/bifrost-file';
import type {
  BifrostFileType,
  ExportRequest,
  ExportRulesRequest,
  ExportNetworkRequest,
  ExportScriptRequest,
  ExportValuesRequest,
  ExportTemplateRequest,
} from '../api/bifrost-file';

export function useExportBifrost() {
  const exportFile = useCallback(
    async (
      fileType: BifrostFileType,
      request: ExportRequest,
      customFilename?: string
    ) => {
      try {
        let content: string;
        const emptyExportMessage = getEmptyExportMessage(fileType, request);
        if (emptyExportMessage) {
          message.warning(emptyExportMessage);
          return;
        }

        const count = getExportItemCount(fileType, request);

        switch (fileType) {
          case 'rules':
            content = await exportRules(request as ExportRulesRequest);
            break;
          case 'network':
            content = await exportNetwork(request as ExportNetworkRequest);
            break;
          case 'script':
            content = await exportScripts(request as ExportScriptRequest);
            break;
          case 'values': {
            const valuesReq = request as ExportValuesRequest;
            content = await exportValues(valuesReq);
            break;
          }
          case 'template': {
            const templateReq = request as ExportTemplateRequest;
            content = await exportTemplates(templateReq);
            break;
          }
          default:
            throw new Error(`Unknown file type: ${fileType}`);
        }

        const filename = customFilename || formatExportFilename(fileType, count);
        downloadFile(content, filename);
        message.success(`Exported as ${filename}`);
      } catch (error) {
        message.error(`Export failed: ${formatBifrostFileError(error)}`);
      }
    },
    []
  );

  return { exportFile };
}

export default useExportBifrost;
