export const IOS_WIFI_PROXY_SSID_STORAGE_KEY = "bifrost.iosWifiProxySsid";
export const IOS_WIFI_PROXY_SSID_EVENT = "bifrost:ios-wifi-proxy-ssid";

export function normalizeIosWifiProxySsid(value: string | null | undefined): string {
  return (value ?? "").trim();
}

export function getSharedIosWifiProxySsid(): string {
  if (typeof window === "undefined") {
    return "";
  }
  try {
    return normalizeIosWifiProxySsid(
      window.localStorage.getItem(IOS_WIFI_PROXY_SSID_STORAGE_KEY),
    );
  } catch {
    return "";
  }
}

export function setSharedIosWifiProxySsid(value: string): string {
  const ssid = normalizeIosWifiProxySsid(value);
  if (typeof window === "undefined") {
    return ssid;
  }
  try {
    if (ssid) {
      window.localStorage.setItem(IOS_WIFI_PROXY_SSID_STORAGE_KEY, ssid);
    } else {
      window.localStorage.removeItem(IOS_WIFI_PROXY_SSID_STORAGE_KEY);
    }
  } catch {
    // Ignore storage failures; the same-tab event still keeps mounted panels in sync.
  }
  window.dispatchEvent(new CustomEvent(IOS_WIFI_PROXY_SSID_EVENT, { detail: ssid }));
  return ssid;
}

export function subscribeSharedIosWifiProxySsid(listener: (ssid: string) => void): () => void {
  if (typeof window === "undefined") {
    return () => {};
  }

  const handleCustomEvent = (event: Event) => {
    listener(normalizeIosWifiProxySsid((event as CustomEvent<string>).detail));
  };
  const handleStorageEvent = (event: StorageEvent) => {
    if (event.key === IOS_WIFI_PROXY_SSID_STORAGE_KEY) {
      listener(normalizeIosWifiProxySsid(event.newValue));
    }
  };

  window.addEventListener(IOS_WIFI_PROXY_SSID_EVENT, handleCustomEvent);
  window.addEventListener("storage", handleStorageEvent);
  return () => {
    window.removeEventListener(IOS_WIFI_PROXY_SSID_EVENT, handleCustomEvent);
    window.removeEventListener("storage", handleStorageEvent);
  };
}
