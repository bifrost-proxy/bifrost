import { get, post } from './client';

export interface CliInstallStatus {
  installed: boolean;
  install_path: string;
  install_dir: string;
  current_exe: string;
  in_path: boolean;
  path_hint: string | null;
  skills_installed: boolean | null;
  skills_message: string | null;
  dry_run: boolean;
}

export interface CliInstallRequest {
  install_dir?: string;
  install_skills?: boolean;
  dry_run?: boolean;
}

export function getCliInstallStatus(): Promise<CliInstallStatus> {
  return get<CliInstallStatus>('/system/cli-install');
}

export function installCliFromDesktop(
  request: CliInstallRequest = {},
): Promise<CliInstallStatus> {
  return post<CliInstallStatus>('/system/cli-install', request);
}
