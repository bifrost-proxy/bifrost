import { get, post } from "./client";
import type { NativeAppInstallResponse, NativeAppStatus } from "../types";

export async function getNativeAppStatus(): Promise<NativeAppStatus> {
  return get<NativeAppStatus>("/system/native-app");
}

export async function installNativeApp(): Promise<NativeAppInstallResponse> {
  return post<NativeAppInstallResponse>("/system/native-app/install");
}
