import { Card, Radio, Space, Typography } from "antd";
import { useI18n, type Locale } from "../../../i18n";

const { Text } = Typography;

const LANGUAGE_OPTIONS: { value: Locale; label: string }[] = [
  { value: "zh-CN", label: "简体中文" },
  { value: "en", label: "English" },
];

export default function LanguageTab() {
  const { t, locale, setLocale } = useI18n();

  return (
    <Card title={t("settings.languageTitle")} size="small">
      <Space direction="vertical" size={12}>
        <Text type="secondary">{t("settings.languageDescription")}</Text>
        <Radio.Group
          data-testid="settings-language-select"
          optionType="button"
          buttonStyle="solid"
          value={locale}
          options={LANGUAGE_OPTIONS}
          onChange={(event) => setLocale(event.target.value as Locale)}
        />
      </Space>
    </Card>
  );
}
