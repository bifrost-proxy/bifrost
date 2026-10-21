import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Locale = "zh-CN" | "en";

export const SUPPORTED_LOCALES: Locale[] = ["zh-CN", "en"];
export const DEFAULT_LOCALE: Locale = "zh-CN";
export const LOCALE_STORAGE_KEY = "bifrost-locale";

interface LocaleState {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}

export const isLocale = (value: unknown): value is Locale =>
  typeof value === "string" && (SUPPORTED_LOCALES as string[]).includes(value);

export const useLocaleStore = create<LocaleState>()(
  persist(
    (set) => ({
      locale: DEFAULT_LOCALE,
      setLocale: (locale) => set({ locale: isLocale(locale) ? locale : DEFAULT_LOCALE }),
    }),
    {
      name: LOCALE_STORAGE_KEY,
      partialize: (state) => ({ locale: state.locale }),
      merge: (persisted, current) => {
        const locale = (persisted as Partial<LocaleState> | undefined)?.locale;
        return { ...current, locale: isLocale(locale) ? locale : DEFAULT_LOCALE };
      },
    },
  ),
);
