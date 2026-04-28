import { apiFetch } from "./apiFetch";

export type KeepAwakeMode = "off" | "auto" | "force_on";

export interface KeepAwakeStatus {
  supported: boolean;
  platform: string;
  mode: KeepAwakeMode;
  active: boolean;
  active_since_secs: number | null;
  on_battery: boolean;
  battery_warning: string | null;
}

export async function getPowerStatus(): Promise<KeepAwakeStatus> {
  const resp = await apiFetch("/api/power/status", { method: "GET" });
  if (!resp.ok) {
    throw new Error(`getPowerStatus failed: ${resp.status}`);
  }
  return (await resp.json()) as KeepAwakeStatus;
}

export async function setPowerOn(): Promise<KeepAwakeStatus> {
  const resp = await apiFetch("/api/power/on", { method: "POST" });
  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`setPowerOn failed: ${resp.status} ${body}`);
  }
  return (await resp.json()) as KeepAwakeStatus;
}

export async function setPowerOff(): Promise<KeepAwakeStatus> {
  const resp = await apiFetch("/api/power/off", { method: "POST" });
  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`setPowerOff failed: ${resp.status} ${body}`);
  }
  return (await resp.json()) as KeepAwakeStatus;
}

export async function setPowerMode(mode: KeepAwakeMode): Promise<KeepAwakeStatus> {
  const resp = await apiFetch("/api/power/mode", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ mode }),
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`setPowerMode failed: ${resp.status} ${body}`);
  }
  return (await resp.json()) as KeepAwakeStatus;
}
