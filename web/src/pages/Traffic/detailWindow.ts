export type TrafficDetailWindowOpenResult =
  | { kind: "desktop" }
  | { kind: "browser"; popup: Window | null };

interface OpenTrafficDetailWindowOptions {
  desktop: boolean;
  recordId: string;
  popupId: string;
  url: string;
  existingPopup: Window | null;
  openDesktop(recordId: string, popupId: string): Promise<void>;
  openBrowser(url: string): Window | null;
}

export async function openTrafficDetailWindow({
  desktop,
  recordId,
  popupId,
  url,
  existingPopup,
  openDesktop,
  openBrowser,
}: OpenTrafficDetailWindowOptions): Promise<TrafficDetailWindowOpenResult> {
  if (desktop) {
    await openDesktop(recordId, popupId);
    return { kind: "desktop" };
  }

  if (existingPopup && !existingPopup.closed) {
    existingPopup.location.href = url;
    existingPopup.focus();
    return { kind: "browser", popup: existingPopup };
  }

  const popup = openBrowser(url);
  popup?.focus();
  return { kind: "browser", popup };
}
