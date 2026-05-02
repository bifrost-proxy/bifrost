import { del, get, patch, post } from "./client";
import type {
  SkillCreateRequest,
  SkillDetailResponse,
  SkillListResponse,
  SkillPatchRequest,
  SkillRecord,
  SkillScope,
  SkillTestReport,
  JsonValue,
} from "../pages/Settings/tabs/agent/types";

const PREFIX = "/im-gateway/agent/skills";

export function listSkills(): Promise<SkillListResponse> {
  return get<SkillListResponse>(PREFIX);
}

export function getSkill(name: string): Promise<SkillDetailResponse> {
  return get<SkillDetailResponse>(`${PREFIX}/${encodeURIComponent(name)}`);
}

export function createSkill(request: SkillCreateRequest): Promise<{ record: SkillRecord }> {
  return post<{ record: SkillRecord }>(PREFIX, request);
}

export function patchSkill(
  name: string,
  request: SkillPatchRequest,
): Promise<{ record: SkillRecord | null }> {
  return patch<{ record: SkillRecord | null }>(`${PREFIX}/${encodeURIComponent(name)}`, request);
}

export function deleteSkill(name: string, scope?: SkillScope): Promise<void> {
  const query = scope ? `?scope=${encodeURIComponent(scope)}` : "";
  return del<void>(`${PREFIX}/${encodeURIComponent(name)}${query}`);
}

export function testSkill(
  name: string,
  inputs: JsonValue,
  timeoutMs?: number,
): Promise<SkillTestReport> {
  return post<SkillTestReport>(`${PREFIX}/${encodeURIComponent(name)}/test`, {
    inputs,
    timeout_ms: timeoutMs,
  });
}

export function packageSkill(
  name: string,
): Promise<{ download_url: string; expires_at_unix: number; bytes?: number }> {
  return post<{ download_url: string; expires_at_unix: number; bytes?: number }>(
    `${PREFIX}/${encodeURIComponent(name)}/package`,
  );
}

export function importSkill(path: string, scope: SkillScope): Promise<{ record: SkillRecord }> {
  return post<{ record: SkillRecord }>(`${PREFIX}/import`, { path, scope });
}

export function validateSkill(request: SkillCreateRequest): Promise<{ ok: boolean; warnings: JsonValue[] }> {
  return post<{ ok: boolean; warnings: JsonValue[] }>(`${PREFIX}/validate`, request);
}
