import { afterEach, describe, expect, it } from "vitest";
import en from "./locales/en";
import zhCN from "./locales/zh-CN";
import {
  DEFAULT_LOCALE,
  LOCALE_STORAGE_KEY,
  t,
  translate,
  useLocaleStore,
} from "./index";

describe("i18n", () => {
  afterEach(() => {
    useLocaleStore.getState().setLocale("en");
  });

  it("defaults to Simplified Chinese", () => {
    expect(DEFAULT_LOCALE).toBe("zh-CN");
  });

  it("translates with the current locale and persists the choice", () => {
    useLocaleStore.getState().setLocale("zh-CN");
    expect(t("common.save")).toBe("保存");
    expect(window.localStorage.getItem(LOCALE_STORAGE_KEY)).toContain(
      '"zh-CN"',
    );

    useLocaleStore.getState().setLocale("en");
    expect(t("common.save")).toBe("Save");
  });

  it("interpolates params and keeps unknown placeholders", () => {
    expect(translate("en", "common.save", { unused: 1 })).toBe("Save");
    expect(translate("en", "missing.key" as never)).toBe("missing.key");
  });

  it("keeps en and zh-CN dictionaries in sync", () => {
    const enTable = en as Record<string, Record<string, string>>;
    const zhTable = zhCN as Record<string, Record<string, string>>;
    expect(Object.keys(zhTable).sort()).toEqual(Object.keys(enTable).sort());
    for (const ns of Object.keys(enTable)) {
      expect(Object.keys(zhTable[ns]).sort(), ns).toEqual(
        Object.keys(enTable[ns]).sort(),
      );
      for (const [key, value] of Object.entries(zhTable[ns])) {
        expect(value.trim(), `${ns}.${key}`).not.toBe("");
        const placeholders = (text: string) => (text.match(/\{\w+\}/g) ?? []).sort();
        expect(placeholders(value), `${ns}.${key}`).toEqual(placeholders(enTable[ns][key]));
      }
    }
  });
});

describe("localizeBackendMessage", () => {
  it("localizes known backend device text only for zh-CN", async () => {
    const { localizeBackendMessage } = await import("./backendMessages");
    expect(localizeBackendMessage("zh-CN", "Installed and trusted")).toBe(
      "已安装并信任",
    );
    expect(
      localizeBackendMessage("zh-CN", "ADB found 2 Android device(s)."),
    ).toBe("ADB 发现 2 台 Android 设备。");
    expect(localizeBackendMessage("en", "Installed and trusted")).toBe(
      "Installed and trusted",
    );
    expect(localizeBackendMessage("zh-CN", "something new")).toBe(
      "something new",
    );
    expect(localizeBackendMessage("zh-CN", undefined)).toBeUndefined();
  });
});
