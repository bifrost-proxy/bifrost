import { useCallback } from "react";
import en from "./locales/en";
import zhCN from "./locales/zh-CN";
import { useLocaleStore, type Locale } from "./store";

export { useLocaleStore, SUPPORTED_LOCALES, DEFAULT_LOCALE, LOCALE_STORAGE_KEY, isLocale } from "./store";
export type { Locale } from "./store";

type Dictionary = typeof en;
type Namespace = keyof Dictionary;

/** Every valid `namespace.key` pair, e.g. `"common.save"`. */
export type TranslationKey = {
  [N in Namespace]: `${N & string}.${keyof Dictionary[N] & string}`;
}[Namespace];

export type TranslationParams = Record<string, string | number | null | undefined>;

const dictionaries: Record<Locale, Record<string, Record<string, string>>> = {
  en: en as unknown as Record<string, Record<string, string>>,
  "zh-CN": zhCN as unknown as Record<string, Record<string, string>>,
};

const lookup = (locale: Locale, key: string): string | undefined => {
  const dot = key.indexOf(".");
  if (dot <= 0) return undefined;
  return dictionaries[locale][key.slice(0, dot)]?.[key.slice(dot + 1)];
};

const interpolate = (template: string, params?: TranslationParams): string => {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params[name];
    return value === undefined || value === null ? match : String(value);
  });
};

/** Translate for an explicit locale. Falls back to English, then to the key. */
export const translate = (
  locale: Locale,
  key: TranslationKey,
  params?: TranslationParams,
): string => interpolate(lookup(locale, key) ?? lookup("en", key) ?? key, params);

/**
 * Translate with the current locale. Safe to call outside React (event
 * handlers, stores, services). Inside components prefer `useI18n()` so the
 * component re-renders when the language changes.
 */
export const t = (key: TranslationKey, params?: TranslationParams): string =>
  translate(useLocaleStore.getState().locale, key, params);

export type TFunction = typeof t;

export const getLocale = (): Locale => useLocaleStore.getState().locale;

export function useI18n() {
  const locale = useLocaleStore((state) => state.locale);
  const setLocale = useLocaleStore((state) => state.setLocale);
  const tr = useCallback(
    (key: TranslationKey, params?: TranslationParams) => translate(locale, key, params),
    [locale],
  );
  return { t: tr, locale, setLocale };
}
