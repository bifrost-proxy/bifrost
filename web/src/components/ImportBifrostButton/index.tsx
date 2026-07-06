import { useCallback } from 'react';
import { Button, Upload, message } from 'antd';
import { ImportOutlined } from '@ant-design/icons';
import type { UploadProps, ButtonProps } from 'antd';
import type { RcFile } from 'antd/es/upload';
import {
  formatBifrostFileError,
  formatImportSuccessMessage,
  getImportedItemCount,
  importFile,
  previewFile,
} from '../../api/bifrost-file';
import type { BifrostFileType, ImportResponse } from '../../api/bifrost-file';
import { useTrafficStore } from '../../stores/useTrafficStore';
import { confirmBifrostFileImport } from '../BifrostFilePreview';

interface ImportBifrostButtonProps {
  expectedType?: BifrostFileType;
  onImportSuccess?: (result: ImportResponse) => void;
  buttonText?: string;
  buttonType?: ButtonProps['type'];
  size?: ButtonProps['size'];
  icon?: React.ReactNode;
  testId?: string;
}

export const ImportBifrostButton: React.FC<ImportBifrostButtonProps> = ({
  expectedType,
  onImportSuccess,
  buttonText = 'Import',
  buttonType = 'default',
  size = 'middle',
  icon = <ImportOutlined />,
  testId,
}) => {
  const handleBeforeUpload: UploadProps['beforeUpload'] = useCallback(
    async (file: RcFile) => {
      if (!file.name.endsWith('.bifrost')) {
        message.error('Please select a .bifrost file');
        return Upload.LIST_IGNORE;
      }

      try {
        const content = await file.text();
        const preview = await previewFile(content);

        if (expectedType && preview.file_type !== expectedType) {
          message.error(
            `File type mismatch: expected ${expectedType}, got ${preview.file_type}`
          );
          return Upload.LIST_IGNORE;
        }

        const confirmed = await confirmBifrostFileImport(file.name, preview);
        if (!confirmed) {
          return Upload.LIST_IGNORE;
        }

        const result = await importFile(content);
        const importedCount = getImportedItemCount(result);

        if (importedCount === 0) {
          message.warning(formatImportSuccessMessage(result, file.name));
          return Upload.LIST_IGNORE;
        } else if (result.warnings && result.warnings.length > 0) {
          message.warning(`Import completed with ${result.warnings.length} warning(s)`);
        } else {
          message.success(formatImportSuccessMessage(result, file.name));
        }

        if (result.file_type === 'network') {
          const trafficStore = useTrafficStore.getState();
          trafficStore.setToolbarFilters({
            ...trafficStore.toolbarFilters,
            imported: ['Imported'],
          });
        }

        onImportSuccess?.(result);
      } catch (error) {
        message.error(`Import failed: ${formatBifrostFileError(error)}`);
      }

      return Upload.LIST_IGNORE;
    },
    [expectedType, onImportSuccess]
  );

  return (
    <Upload
      accept=".bifrost"
      showUploadList={false}
      beforeUpload={handleBeforeUpload}
      multiple
      data-testid={testId}
    >
      <Button type={buttonType} size={size} icon={icon} data-testid={testId ? `${testId}-button` : undefined}>
        {buttonText}
      </Button>
    </Upload>
  );
};

export default ImportBifrostButton;
