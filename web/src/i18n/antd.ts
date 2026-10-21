import dayjs from "dayjs";
import "dayjs/locale/zh-cn";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";
import type { Locale as AntdLocale } from "antd/es/locale";
import { useEffect } from "react";
import { useLocaleStore, type Locale } from "./store";

const ANTD_LOCALES: Record<Locale, AntdLocale> = {
  en: enUS,
  "zh-CN": zhCN,
};

const DAYJS_LOCALES: Record<Locale, string> = {
  en: "en",
  "zh-CN": "zh-cn",
};

export const getAntdLocale = (locale: Locale): AntdLocale => ANTD_LOCALES[locale];

const applyGlobalLocale = (locale: Locale) => {
  dayjs.locale(DAYJS_LOCALES[locale]);
  if (typeof document !== "undefined") {
    document.documentElement.setAttribute("lang", locale);
  }
};

applyGlobalLocale(useLocaleStore.getState().locale);

/** Keeps dayjs and `<html lang>` in sync and returns the antd locale pack. */
export function useAntdLocale(): AntdLocale {
  const locale = useLocaleStore((state) => state.locale);
  useEffect(() => {
    applyGlobalLocale(locale);
  }, [locale]);
  return ANTD_LOCALES[locale];
}
