import { useCallback } from 'react';
import { Button, Upload, message } from 'antd';
import { ImportOutlined } from '@ant-design/icons';
import type { UploadProps, ButtonProps } from 'antd';
import type { RcFile } from 'antd/es/upload';
import { importFile, detectType } from '../../api/bifrost-file';
import type { BifrostFileType, ImportResponse } from '../../api/bifrost-file';
import { useTrafficStore } from '../../stores/useTrafficStore';

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
  buttonText = '导入',
  buttonType = 'default',
  size = 'middle',
  icon = <ImportOutlined />,
  testId,
}) => {
  const handleBeforeUpload: UploadProps['beforeUpload'] = useCallback(
    async (file: RcFile) => {
      if (!file.name.endsWith('.bifrost')) {
        message.error('请选择 .bifrost 格式的文件');
        return Upload.LIST_IGNORE;
      }

      try {
        const content = await file.text();

        if (expectedType) {
          const detected = await detectType(content);
          if (detected.file_type !== expectedType) {
            message.error(
              `文件类型不匹配: 期望 ${expectedType}，实际为 ${detected.file_type}`
            );
            return Upload.LIST_IGNORE;
          }
        }

        const result = await importFile(content);

        if (result.warnings && result.warnings.length > 0) {
          message.warning(`导入完成，有 ${result.warnings.length} 条警告`);
        } else {
          message.success('导入成功');
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
        message.error(`导入失败: ${error}`);
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
