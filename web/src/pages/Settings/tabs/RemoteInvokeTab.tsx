import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import {
  Alert,
  Badge,
  Button,
  Card,
  Col,
  Descriptions,
  Divider,
  Empty,
  Form,
  Input,
  InputNumber,
  List,
  Modal,
  Popconfirm,
  Radio,
  Row,
  Select,
  Space,
  Spin,
  Switch,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";
import {
  ApiOutlined,
  CodeOutlined,
  CopyOutlined,
  DeleteOutlined,
  DisconnectOutlined,
  EditOutlined,
  EyeOutlined,
  FileOutlined,
  HistoryOutlined,
  KeyOutlined,
  PlusOutlined,
  ReloadOutlined,
  SafetyOutlined,
  ScanOutlined,
  SearchOutlined,
  SettingOutlined,
  StopOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import {
  clearCalls,
  createRemoteInvokeSshKey,
  enterDiscoveryMode,
  exitDiscoveryMode,
  getClientIdentity,
  getRemoteInvokeStatus,
  getRemoteInvokeSshKey,
  getRemoteInvokeSshPrivateKey,
  getRemoteShellConfig,
  listCalls,
  listGrants,
  refreshPairCode,
  resetRemoteInvokeSshKey,
  revokeGrant,
  revokeRemoteInvokeSshKey,
  updateGrant,
  updateRemoteShellConfig,
  getCallArgsPreviewSource,
  getCall,
  type CreateRemoteInvokeSshKeyInput,
  type Call,
  type ClientIdentity,
  type DiscoverySession,
  type Grant,
  type GrantMode,
  type RemoteInvokeSshCallerInfo,
  type RemoteInvokeSshKeyRecord,
  type RemoteInvokeSshKeySecretPayload,
  type RemoteInvokeStatus,
  type RemoteShellSet,
  type FileAccessScope,
  getFileAccessConfig,
  updateFileAccessConfig,
  type FileAccessConfig,
  type FileAccessGrantPolicy,
  type FileOp,
  ALL_FILE_OPS,
  FILE_READ_OPS,
} from "../../../api/remoteInvoke";
import { isConnectionIssueError, isNotFoundError } from "../../../api/client";
import { copyToClipboard } from "../../../utils/clipboard";
import { usePairingRequestStore } from "../../../stores/usePairingRequestStore";
import PairingRequestModal from "../../../components/PairingRequestModal";
import type { PairingRequest } from "../../../api/remoteInvoke";

const { Text, Title } = Typography;
const { TextArea } = Input;
const { Option } = Select;

const recentCallItemStyle: CSSProperties = {
  cursor: "pointer",
  display: "block",
  paddingInline: 0,
};

const recentCallRowStyle: CSSProperties = {
  display: "grid",
  gridTemplateColumns: "minmax(0, 1fr) minmax(0, 500px) auto",
  alignItems: "center",
  gap: 8,
  width: "100%",
};

const recentCallPrimaryStyle: CSSProperties = {
  minWidth: 0,
  display: "flex",
  alignItems: "center",
  gap: 8,
  overflow: "hidden",
};

const recentCallMetaStyle: CSSProperties = {
  minWidth: 0,
  display: "flex",
  alignItems: "center",
  gap: 6,
  overflow: "hidden",
};

const recentCallTextEllipsisStyle: CSSProperties = {
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const callDetailPreStyle: CSSProperties = {
  margin: 0,
  maxHeight: 220,
  overflow: "auto",
  whiteSpace: "pre-wrap",
  wordBreak: "break-word",
};

const SSH_GRANT_MODE: { label: string; value: GrantMode } = {
  label: "Permanent",
  value: "permanent",
};

function formatFingerprint(fp: string): string {
  if (!fp || fp.length < 16) return fp || "-";
  const short = fp.slice(0, 16);
  return `${short.slice(0, 4)}:${short.slice(4, 8)}:${short.slice(8, 12)}:${short.slice(12, 16)}`;
}

function formatArgsPreview(argsJson?: string | null): string {
  if (!argsJson) return "";
  try {
    const obj = JSON.parse(argsJson);
    return Object.entries(obj)
      .filter(([, v]) => v != null)
      .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join(" ");
  } catch {
    return argsJson.length > 60 ? argsJson.slice(0, 60) + "…" : argsJson;
  }
}

function getCallCommandPreview(call: Call): string {
  const summaryPreview = call.command_summary?.command_preview?.trim();
  const decryptedPreview = call.command?.command?.trim();
  const routeOnlyPreview =
    summaryPreview && call.command_kind && summaryPreview === call.command_kind;

  return (
    (!routeOnlyPreview && summaryPreview) ||
    decryptedPreview ||
    summaryPreview ||
    call.command_kind ||
    "-"
  );
}

function formatJsonForDisplay(value: unknown): string {
  if (value == null) return "-";
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return "-";
    try {
      return JSON.stringify(JSON.parse(trimmed), null, 2);
    } catch {
      return value;
    }
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function renderCallDetailBlock(value: unknown) {
  return (
    <pre style={callDetailPreStyle}>
      <Text code>{formatJsonForDisplay(value)}</Text>
    </pre>
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function getStringArrayField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): string[] {
  const value = metadata?.[key];
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string");
}

function getStringField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): string | undefined {
  const value = metadata?.[key];
  return typeof value === "string" && value.trim() ? value : undefined;
}

function getNumberField(
  metadata: Record<string, unknown> | undefined,
  key: string,
): number | undefined {
  const value = metadata?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

type ShellExecMode = "argv_exec" | "shell_text";
type ShellAccessMode = "sandbox" | "full-access" | "custom";

interface ShellPolicyEditorItem {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  profile_id?: string;
  exec_mode: ShellExecMode;
  allowed_executables: string[];
  allowed_shell_patterns: string[];
  cwd_allowlist: string[];
  env_allowlist: string[];
  default_cwd: string;
  max_timeout_ms: number | null;
  stdin_allowed: boolean;
  interactive_allowed: boolean;
  extra_metadata: Record<string, unknown>;
}

interface ShellProfileEditorItem {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  cwd_allowlist: string[];
  env_allowlist: string[];
  default_cwd: string;
  max_timeout_ms: number | null;
  stdin_allowed: boolean;
  interactive_allowed: boolean;
  extra_metadata: Record<string, unknown>;
}

interface ShellAccessCapability {
  allowed: boolean;
  text: string;
}

interface ShellAccessPresetDefinition {
  mode: Exclude<ShellAccessMode, "custom">;
  label: string;
  description: string;
  capabilities: ShellAccessCapability[];
  policies: ShellPolicyEditorItem[];
  profiles: ShellProfileEditorItem[];
}

const SHELL_POLICY_METADATA_KEYS = [
  "exec_mode",
  "allowed_executables",
  "allowed_shell_patterns",
  "cwd_allowlist",
  "env_allowlist",
  "default_cwd",
  "max_timeout_ms",
  "stdin_allowed",
  "interactive_allowed",
];

const SHELL_PROFILE_METADATA_KEYS = [
  "cwd_allowlist",
  "env_allowlist",
  "default_cwd",
  "max_timeout_ms",
  "stdin_allowed",
  "interactive_allowed",
];

function omitKnownMetadata(
  metadata: Record<string, unknown> | undefined,
  knownKeys: string[],
): Record<string, unknown> {
  if (!metadata) return {};
  return Object.fromEntries(
    Object.entries(metadata).filter(([key]) => !knownKeys.includes(key)),
  );
}

function nextShellItemId(prefix: string, existingIds: string[]): string {
  const seen = new Set(existingIds.map((item) => item.trim()).filter(Boolean));
  let nextIndex = existingIds.length + 1;
  let nextId = `${prefix}-${nextIndex}`;

  while (seen.has(nextId)) {
    nextIndex += 1;
    nextId = `${prefix}-${nextIndex}`;
  }

  return nextId;
}

function toShellPolicyEditorItem(
  policy: RemoteShellSet["policies"][number],
): ShellPolicyEditorItem {
  const metadata = isRecord(policy.metadata) ? policy.metadata : {};
  return {
    id: policy.id,
    name: policy.name,
    description: policy.description ?? "",
    enabled: policy.enabled,
    profile_id: policy.profile_id ?? undefined,
    exec_mode:
      getStringField(metadata, "exec_mode") === "shell_text"
        ? "shell_text"
        : "argv_exec",
    allowed_executables: getStringArrayField(metadata, "allowed_executables"),
    allowed_shell_patterns: getStringArrayField(metadata, "allowed_shell_patterns"),
    cwd_allowlist: getStringArrayField(metadata, "cwd_allowlist"),
    env_allowlist: getStringArrayField(metadata, "env_allowlist"),
    default_cwd: getStringField(metadata, "default_cwd") ?? "",
    max_timeout_ms: getNumberField(metadata, "max_timeout_ms") ?? null,
    stdin_allowed: Boolean(metadata.stdin_allowed),
    interactive_allowed: Boolean(metadata.interactive_allowed),
    extra_metadata: omitKnownMetadata(metadata, SHELL_POLICY_METADATA_KEYS),
  };
}

function toShellProfileEditorItem(
  profile: RemoteShellSet["profiles"][number],
): ShellProfileEditorItem {
  const metadata = isRecord(profile.metadata) ? profile.metadata : {};
  return {
    id: profile.id,
    name: profile.name,
    description: profile.description ?? "",
    enabled: profile.enabled,
    cwd_allowlist: getStringArrayField(metadata, "cwd_allowlist"),
    env_allowlist: getStringArrayField(metadata, "env_allowlist"),
    default_cwd: getStringField(metadata, "default_cwd") ?? "",
    max_timeout_ms: getNumberField(metadata, "max_timeout_ms") ?? null,
    stdin_allowed: Boolean(metadata.stdin_allowed),
    interactive_allowed: Boolean(metadata.interactive_allowed),
    extra_metadata: omitKnownMetadata(metadata, SHELL_PROFILE_METADATA_KEYS),
  };
}

function buildShellPolicyMetadata(
  item: ShellPolicyEditorItem,
): Record<string, unknown> {
  const metadata: Record<string, unknown> = {
    ...item.extra_metadata,
    exec_mode: item.exec_mode,
  };

  if (item.allowed_executables.length > 0) {
    metadata.allowed_executables = item.allowed_executables;
  }
  if (item.allowed_shell_patterns.length > 0) {
    metadata.allowed_shell_patterns = item.allowed_shell_patterns;
  }
  if (item.cwd_allowlist.length > 0) {
    metadata.cwd_allowlist = item.cwd_allowlist;
  }
  if (item.env_allowlist.length > 0) {
    metadata.env_allowlist = item.env_allowlist;
  }
  if (item.default_cwd.trim()) {
    metadata.default_cwd = item.default_cwd.trim();
  }
  if (item.max_timeout_ms != null) {
    metadata.max_timeout_ms = item.max_timeout_ms;
  }
  if (item.stdin_allowed) {
    metadata.stdin_allowed = true;
  }
  if (item.interactive_allowed) {
    metadata.interactive_allowed = true;
  }

  return metadata;
}

function buildShellProfileMetadata(
  item: ShellProfileEditorItem,
): Record<string, unknown> {
  const metadata: Record<string, unknown> = {
    ...item.extra_metadata,
  };

  if (item.cwd_allowlist.length > 0) {
    metadata.cwd_allowlist = item.cwd_allowlist;
  }
  if (item.env_allowlist.length > 0) {
    metadata.env_allowlist = item.env_allowlist;
  }
  if (item.default_cwd.trim()) {
    metadata.default_cwd = item.default_cwd.trim();
  }
  if (item.max_timeout_ms != null) {
    metadata.max_timeout_ms = item.max_timeout_ms;
  }
  if (item.stdin_allowed) {
    metadata.stdin_allowed = true;
  }
  if (item.interactive_allowed) {
    metadata.interactive_allowed = true;
  }

  return metadata;
}

const DEFAULT_SANDBOX_ENV_KEYS = ["PATH", "LANG", "LC_ALL", "TERM"];
const DEFAULT_SANDBOX_ENV = {
  PATH: "/usr/bin:/bin:/usr/sbin:/sbin",
  LANG: "C.UTF-8",
  LC_ALL: "C.UTF-8",
  TERM: "xterm-256color",
};

function createSandboxPreset(): ShellAccessPresetDefinition {
  return {
    mode: "sandbox",
    label: "Safe Sandbox (coming soon)",
    description:
      "A future locked-down execution mode. Until the sandbox runtime lands, Bifrost will reject shell execution attempts for this preset.",
    capabilities: [
      { allowed: true,  text: "Reserved for upcoming isolated sandbox runtime" },
      { allowed: false, text: "Does not run any commands today (rejects with a clear reason)" },
      { allowed: false, text: "No stdin / no interactive shells" },
      { allowed: false, text: "No sudo / no system modification" },
    ],
    profiles: [
      {
        id: "default-sandbox",
        name: "Default Sandbox",
        description: "Baseline sandboxed shell execution.",
        enabled: true,
        cwd_allowlist: [],
        env_allowlist: [...DEFAULT_SANDBOX_ENV_KEYS],
        default_cwd: "",
        max_timeout_ms: 30000,
        stdin_allowed: false,
        interactive_allowed: false,
        extra_metadata: {
          inherit_env: false,
          default_env: DEFAULT_SANDBOX_ENV,
          reject_reason:
            "sandbox execution is not implemented yet on this target; choose Trusted (Full Access) or Custom Rules",
        },
      },
    ],
    policies: [
      {
        id: "default-sandbox",
        name: "Default Sandbox",
        description: "Run shell text inside the default sandbox constraints.",
        enabled: true,
        profile_id: "default-sandbox",
        exec_mode: "shell_text",
        allowed_executables: [],
        allowed_shell_patterns: ["^(?s:.*)$"],
        cwd_allowlist: [],
        env_allowlist: [],
        default_cwd: "",
        max_timeout_ms: null,
        stdin_allowed: false,
        interactive_allowed: false,
        extra_metadata: {
          shell: "/bin/bash",
          reject_reason:
            "sandbox execution is not implemented yet on this target; choose Trusted (Full Access) or Custom Rules",
        },
      },
    ],
  };
}

function createFullAccessPreset(): ShellAccessPresetDefinition {
  return {
    mode: "full-access",
    label: "Trusted (Full Access)",
    description:
      "Trust the remote caller with unrestricted shell execution on this device. Only use for agents you fully trust.",
    capabilities: [
      { allowed: true,  text: "Run any command (argv or shell text) with the caller's choice of arguments" },
      { allowed: true,  text: "Inherit this device's environment variables (PATH, tokens, etc.)" },
      { allowed: true,  text: "Open interactive terminals and accept stdin" },
      { allowed: true,  text: "Run long-running commands without a timeout cap" },
      { allowed: false, text: "Still cannot bypass the file-access scope negotiated in the pairing" },
    ],
    profiles: [],
    policies: [
      {
        id: "full-access",
        name: "Full Access",
        description: "Run any argv command or shell text with full caller access.",
        enabled: true,
        profile_id: undefined,
        exec_mode: "shell_text",
        allowed_executables: [],
        allowed_shell_patterns: ["^(?s:.*)$"],
        cwd_allowlist: [],
        env_allowlist: [],
        default_cwd: "",
        max_timeout_ms: null,
        stdin_allowed: true,
        interactive_allowed: true,
        extra_metadata: {
          shell: "/bin/bash",
          inherit_env: true,
          allowed_exec_modes: ["argv_exec", "shell_text"],
          allow_any_executable: true,
        },
      },
    ],
  };
}

const SHELL_ACCESS_PRESETS: ShellAccessPresetDefinition[] = [
  createSandboxPreset(),
  createFullAccessPreset(),
];

function shellPresetDefinition(
  mode: Exclude<ShellAccessMode, "custom">,
): ShellAccessPresetDefinition {
  return mode === "sandbox"
    ? createSandboxPreset()
    : createFullAccessPreset();
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sameShellEditorItems<T>(left: T[], right: T[]): boolean {
  return stableJson(left) === stableJson(right);
}

function inferShellAccessMode(
  profiles: ShellProfileEditorItem[],
  policies: ShellPolicyEditorItem[],
): ShellAccessMode {
  for (const preset of SHELL_ACCESS_PRESETS) {
    if (
      sameShellEditorItems(profiles, preset.profiles) &&
      sameShellEditorItems(policies, preset.policies)
    ) {
      return preset.mode;
    }
  }
  return "custom";
}

function shellAccessModeLabel(mode: ShellAccessMode): string {
  switch (mode) {
    case "sandbox":
      return "Safe Sandbox";
    case "full-access":
      return "Trusted (Full Access)";
    default:
      return "Custom Rules";
  }
}

function formatBytes(bytes: number | null | undefined): string | null {
  if (bytes == null || bytes === 0) return null;
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

function formatTimestamp(value: string | number | null | undefined): string {
  if (value == null || value === "") return "-";
  const date = typeof value === "number" ? new Date(value) : new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function formatSshFingerprint(fingerprint?: string | null): string {
  if (!fingerprint) return "-";
  if (fingerprint.startsWith("SHA256:")) return fingerprint;
  return fingerprint.length > 48 ? `${fingerprint.slice(0, 24)}…` : fingerprint;
}

function formatCallerInfo(info: RemoteInvokeSshCallerInfo | null): string {
  if (!info) return "No SSH caller has connected yet.";

  const headline = [info.hostname, info.username && `as ${info.username}`]
    .filter(Boolean)
    .join(" ");
  const details = [info.source_ip ?? info.ip, info.platform]
    .filter(Boolean)
    .join(" · ");

  return [headline, details].filter(Boolean).join(" — ") || "No SSH caller has connected yet.";
}

function formatSshGrantMode(mode: GrantMode): string {
  return mode === "permanent" ? SSH_GRANT_MODE.label : mode;
}

function formatCountdown(expiresAt: number): string {
  const remaining = Math.max(0, Math.floor((expiresAt - Date.now()) / 1000));
  if (remaining <= 0) return "Expired";
  const m = Math.floor(remaining / 60);
  const s = remaining % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function getGrantAccessMode(grant: Grant): "query" | "selected" | "all" {
  if (grant.grant_scope === "remote_query") {
    return "query";
  }
  const binding = isRecord(grant.policy_binding) ? grant.policy_binding : null;
  if (binding?.mode === "selected") {
    return "selected";
  }
  return "all";
}

function getGrantSelectedPolicies(grant: Grant): string[] {
  const binding = isRecord(grant.policy_binding) ? grant.policy_binding : null;
  if (!binding || binding.mode !== "selected" || !Array.isArray(binding.policy_ids)) {
    return [];
  }
  return binding.policy_ids.filter((item): item is string => typeof item === "string");
}

function getCallStatusColor(call: Call): string {
  switch (call.status) {
    case "completed":
      return call.exit_code === 0 ? "green" : "red";
    case "cancelled":
      return "orange";
    case "streaming":
    case "authorized":
    case "key_exchanged":
    case "pending":
    case "running":
      return "processing";
    case "failed":
    case "timeout":
      return "red";
    default:
      return "default";
  }
}

function renderStateTag(state: string) {
  switch (state.toLowerCase()) {
    case "connected":
      return <Tag color="green">Connected</Tag>;
    case "connecting":
    case "registering":
      return <Tag color="processing">Connecting</Tag>;
    case "reconnecting":
      return <Tag color="orange">Reconnecting</Tag>;
    case "disconnected":
      return <Tag color="default">Disconnected</Tag>;
    default:
      return <Tag>{state}</Tag>;
  }
}

export default function RemoteInvokeTab() {
  const [status, setStatus] = useState<RemoteInvokeStatus | null>(null);
  const [identity, setIdentity] = useState<ClientIdentity | null>(null);
  const [sshKey, setSshKey] = useState<RemoteInvokeSshKeyRecord | null>(null);
  const [sshApiAvailable, setSshApiAvailable] = useState(true);
  const [sshLoading, setSshLoading] = useState(false);
  const [sshAction, setSshAction] = useState<
    "create" | "download" | "reset" | "revoke" | null
  >(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [secretModal, setSecretModal] = useState<{
    title: string;
    description: string;
    payload: RemoteInvokeSshKeySecretPayload;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [discoveryLoading, setDiscoveryLoading] = useState(false);
  const [countdown, setCountdown] = useState("");
  const [grants, setGrants] = useState<Grant[]>([]);
  const [calls, setCalls] = useState<Call[]>([]);
  const [shellConfig, setShellConfig] = useState<RemoteShellSet | null>(null);
  const [shellLoading, setShellLoading] = useState(false);
  const [shellEditorOpen, setShellEditorOpen] = useState(false);
  const [shellEditorMode, setShellEditorMode] = useState<ShellAccessMode>("custom");
  const [shellEditorPolicies, setShellEditorPolicies] = useState<
    ShellPolicyEditorItem[]
  >([]);
  const [shellEditorProfiles, setShellEditorProfiles] = useState<
    ShellProfileEditorItem[]
  >([]);
  const [shellSaveLoading, setShellSaveLoading] = useState(false);
  const [fileAccessConfig, setFileAccessConfig] = useState<FileAccessConfig | null>(null);
  const [fileAccessLoading, setFileAccessLoading] = useState(false);
  const [fileAccessEditorOpen, setFileAccessEditorOpen] = useState(false);
  const [fileAccessEditorGrants, setFileAccessEditorGrants] = useState<FileAccessGrantPolicy[]>([]);
  const [fileAccessSaveLoading, setFileAccessSaveLoading] = useState(false);
  const [grantEditorOpen, setGrantEditorOpen] = useState(false);
  const [editingGrant, setEditingGrant] = useState<Grant | null>(null);
  const [grantEditorPreset, setGrantEditorPreset] = useState<"query" | "full" | "shell_only" | "file_only" | "custom">("full");
  const [grantEditorAccessMode, setGrantEditorAccessMode] = useState<"query" | "selected" | "all">("query");
  const [grantEditorSelectedPolicies, setGrantEditorSelectedPolicies] = useState<string[]>([]);
  const [grantEditorStdinAllowed, setGrantEditorStdinAllowed] = useState(false);
  const [grantEditorInteractiveAllowed, setGrantEditorInteractiveAllowed] = useState(false);
  const [grantEditorFileAccess, setGrantEditorFileAccess] = useState<FileAccessScope>("none");
  const [grantSaveLoading, setGrantSaveLoading] = useState(false);
  const [callDetailOpen, setCallDetailOpen] = useState(false);
  const [callDetailLoading, setCallDetailLoading] = useState(false);
  const [selectedCall, setSelectedCall] = useState<Call | null>(null);
  const pollRef = useRef<number | null>(null);
  const [sshForm] = Form.useForm<CreateRemoteInvokeSshKeyInput>();

  const [reviewPairing, setReviewPairing] = useState<PairingRequest | null>(null);
  const pendingPairings = usePairingRequestStore((s) => s.pendingList);
  const storeFetchPairings = usePairingRequestStore((s) => s.fetchPendingList);
  const enabledShellPolicies = shellConfig?.policies.filter((policy) => policy.enabled) ?? [];

  const refresh = useCallback(async () => {
    try {
      const [s, id] = await Promise.all([
        getRemoteInvokeStatus(),
        getClientIdentity(),
      ]);
      setStatus(s);
      setIdentity(id);
    } catch (e) {
      if (!isConnectionIssueError(e)) {
        console.error("Failed to fetch remote invoke status");
      }
    }
  }, []);

  const refreshGrants = useCallback(async () => {
    try {
      const res = await listGrants();
      const sorted = (res.grants ?? []).sort((a, b) => (b.created_at ?? 0) - (a.created_at ?? 0));
      setGrants(sorted);
    } catch {
      // ignore
    }
  }, []);

  const refreshCalls = useCallback(async () => {
    try {
      const res = await listCalls();
      const sorted = (res.calls ?? []).sort((a, b) => (b.started_at ?? 0) - (a.started_at ?? 0));
      setCalls(sorted);
    } catch {
      // ignore
    }
  }, []);

  const handleClearCalls = useCallback(async () => {
    try {
      const res = await clearCalls();
      setCalls([]);
      setSelectedCall(null);
      message.success(`Cleared ${res.removed ?? 0} recent calls`);
    } catch {
      message.error("Failed to clear recent calls");
    }
  }, []);

  const handleOpenCallDetail = useCallback(async (call: Call) => {
    setSelectedCall(call);
    setCallDetailOpen(true);
    setCallDetailLoading(true);
    try {
      const res = await getCall(call.call_id);
      setSelectedCall(res.call);
    } catch (e) {
      message.warning(
        e instanceof Error
          ? `Failed to refresh call detail: ${e.message}`
          : "Failed to refresh call detail",
      );
    } finally {
      setCallDetailLoading(false);
    }
  }, []);

  const refreshShellConfig = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setShellLoading(true);
      }

      try {
        const config = await getRemoteShellConfig();
        setShellConfig(config);
      } catch (e) {
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : "Failed to load shell access config",
          );
        }
      } finally {
        if (!silent) {
          setShellLoading(false);
        }
      }
    },
    [],
  );

  const refreshFileAccessConfig = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setFileAccessLoading(true);
      }
      try {
        const config = await getFileAccessConfig();
        setFileAccessConfig(config);
      } catch (e) {
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : "Failed to load file access config",
          );
        }
      } finally {
        if (!silent) {
          setFileAccessLoading(false);
        }
      }
    },
    [],
  );

  const refreshSshKey = useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent ?? false;
      if (!silent) {
        setSshLoading(true);
      }

      try {
        const currentKey = await getRemoteInvokeSshKey();
        setSshApiAvailable(true);
        setSshKey(currentKey);
      } catch (e) {
        if (isNotFoundError(e)) {
          setSshApiAvailable(false);
          setSshKey(null);
          return;
        }
        if (!silent && !isConnectionIssueError(e)) {
          message.error(
            e instanceof Error ? e.message : "Failed to load SSH key status",
          );
        }
      } finally {
        if (!silent) {
          setSshLoading(false);
        }
      }
    },
    [],
  );

  useEffect(() => {
    void refresh();
    void refreshGrants();
    void refreshCalls();
    void refreshSshKey();
    void refreshShellConfig();
    void refreshFileAccessConfig();
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey, refreshShellConfig, refreshFileAccessConfig]);

  useEffect(() => {
    pollRef.current = window.setInterval(() => {
      void refresh();
      void refreshGrants();
      void refreshCalls();
      void refreshShellConfig({ silent: true });
      void refreshFileAccessConfig({ silent: true });
      if (sshApiAvailable) {
        void refreshSshKey({ silent: true });
      }
    }, 3000);
    return () => {
      if (pollRef.current) window.clearInterval(pollRef.current);
    };
  }, [refresh, refreshGrants, refreshCalls, refreshSshKey, refreshShellConfig, refreshFileAccessConfig, sshApiAvailable]);

  useEffect(() => {
    const session = status?.discovery_session;
    if (!session) {
      setCountdown("");
      return;
    }
    const tick = () => {
      setCountdown(formatCountdown(session.expires_at));
    };
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [status?.discovery_session]);

  const handleEnterDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await enterDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success("Discovery mode enabled");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to enter discovery mode",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleExitDiscovery = async () => {
    setDiscoveryLoading(true);
    try {
      await exitDiscoveryMode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: null } : prev,
      );
      message.success("Discovery mode disabled");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to exit discovery mode",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleRefreshCode = async () => {
    setDiscoveryLoading(true);
    try {
      const res = await refreshPairCode();
      setStatus((prev) =>
        prev ? { ...prev, discovery_session: res.session } : prev,
      );
      message.success("Pair code refreshed");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to refresh pair code",
      );
    } finally {
      setDiscoveryLoading(false);
    }
  };

  const handleCopyCode = () => {
    const code = status?.discovery_session?.pair_code;
    if (code) {
      copyToClipboard(code);
      message.success("Pair code copied");
    }
  };

  const handleRevokeGrant = async (grantId: string) => {
    try {
      await revokeGrant(grantId);
      message.success("Grant revoked");
      void refreshGrants();
    } catch (e) {
      message.error(e instanceof Error ? e.message : "Failed to revoke grant");
    }
  };

  const openGrantEditor = (grant: Grant) => {
    const accessMode = getGrantAccessMode(grant);
    const fa = grant.file_access || "none";
    const isShell = accessMode === "selected" || accessMode === "all";
    const hasFile = fa === "read" || fa === "read_write";

    // Derive preset from current grant state
    let preset: "query" | "full" | "shell_only" | "file_only" | "custom";
    if (isShell && hasFile && grant.interactive_allowed) {
      preset = "full";
    } else if (isShell && hasFile && !grant.interactive_allowed) {
      preset = "shell_only";
    } else if (!isShell && hasFile) {
      preset = "file_only";
    } else if (!isShell && !hasFile) {
      preset = "query";
    } else {
      preset = "custom";
    }

    setEditingGrant(grant);
    setGrantEditorPreset(preset);
    setGrantEditorAccessMode(accessMode);
    setGrantEditorSelectedPolicies(getGrantSelectedPolicies(grant));
    setGrantEditorStdinAllowed(Boolean(grant.stdin_allowed));
    setGrantEditorInteractiveAllowed(Boolean(grant.interactive_allowed));
    setGrantEditorFileAccess(fa as FileAccessScope);
    setGrantEditorOpen(true);
  };

  const handleSaveGrant = async () => {
    if (!editingGrant) {
      return;
    }
    if (grantEditorPreset === "custom" && grantEditorAccessMode === "selected" && grantEditorSelectedPolicies.length === 0) {
      message.error("Choose at least one shell policy");
      return;
    }

    // Resolve preset to actual grant parameters
    const resolvePayload = () => {
      switch (grantEditorPreset) {
        case "query":
          return {
            grant_scope: "remote_query",
            file_access: "none" as const,
            policy_binding: null,
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "full":
          return {
            grant_scope: "remote_shell_interactive",
            file_access: "read_write" as const,
            policy_binding: { mode: "all" },
            interactive_allowed: true,
            stdin_allowed: true,
          };
        case "shell_only":
          return {
            grant_scope: "remote_shell_exec",
            file_access: "read_write" as const,
            policy_binding: { mode: "all" },
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "file_only":
          return {
            grant_scope: "remote_query",
            file_access: "read_write" as const,
            policy_binding: null,
            interactive_allowed: false,
            stdin_allowed: false,
          };
        case "custom":
          if (grantEditorAccessMode === "query") {
            return {
              grant_scope: "remote_query",
              file_access: grantEditorFileAccess,
              policy_binding: null,
              interactive_allowed: false,
              stdin_allowed: false,
            };
          }
          return {
            grant_scope: grantEditorInteractiveAllowed
              ? "remote_shell_interactive"
              : "remote_shell_exec",
            file_access: grantEditorFileAccess,
            policy_binding:
              grantEditorAccessMode === "all"
                ? { mode: "all" }
                : { mode: "selected", policy_ids: grantEditorSelectedPolicies },
            interactive_allowed: grantEditorInteractiveAllowed,
            stdin_allowed: grantEditorStdinAllowed,
          };
      }
    };

    const payload = resolvePayload();
    setGrantSaveLoading(true);
    try {
      await updateGrant(editingGrant.grant_id, payload);
      message.success("Grant access updated");
      setGrantEditorOpen(false);
      setEditingGrant(null);
      void refreshGrants();
    } catch (e) {
      message.error(e instanceof Error ? e.message : "Failed to update grant");
    } finally {
      setGrantSaveLoading(false);
    }
  };

  const openShellEditor = () => {
    const nextProfiles = shellConfig?.profiles.map(toShellProfileEditorItem) ?? [];
    const nextPolicies = shellConfig?.policies.map(toShellPolicyEditorItem) ?? [];
    setShellEditorProfiles(nextProfiles);
    setShellEditorPolicies(nextPolicies);
    setShellEditorMode(inferShellAccessMode(nextProfiles, nextPolicies));
    setShellEditorOpen(true);
  };

  const applyShellAccessMode = (mode: ShellAccessMode) => {
    setShellEditorMode(mode);
    if (mode === "custom") {
      return;
    }

    const preset = shellPresetDefinition(mode);
    setShellEditorProfiles(preset.profiles);
    setShellEditorPolicies(preset.policies);
  };

  const handleSaveShellConfig = async () => {
    const policyIds = new Set<string>();
    for (const policy of shellEditorPolicies) {
      if (!policy.id.trim()) {
        message.error("Every policy needs an ID");
        return;
      }
      if (!policy.name.trim()) {
        message.error("Every policy needs a name");
        return;
      }
      if (policyIds.has(policy.id.trim())) {
        message.error(`Duplicate policy ID: ${policy.id}`);
        return;
      }
      policyIds.add(policy.id.trim());
    }

    const profileIds = new Set<string>();
    for (const profile of shellEditorProfiles) {
      if (!profile.id.trim()) {
        message.error("Every profile needs an ID");
        return;
      }
      if (!profile.name.trim()) {
        message.error("Every profile needs a name");
        return;
      }
      if (profileIds.has(profile.id.trim())) {
        message.error(`Duplicate profile ID: ${profile.id}`);
        return;
      }
      profileIds.add(profile.id.trim());
    }

    for (const policy of shellEditorPolicies) {
      if (policy.profile_id && !profileIds.has(policy.profile_id)) {
        message.error(`Policy ${policy.name} references a missing profile`);
        return;
      }
    }

    const parsed: RemoteShellSet = {
      schema_version: shellConfig?.schema_version ?? 1,
      version: shellConfig?.version ?? 0,
      policies: shellEditorPolicies.map((policy) => ({
        id: policy.id.trim(),
        name: policy.name.trim(),
        description: policy.description.trim() || null,
        enabled: policy.enabled,
        profile_id: policy.profile_id?.trim() || null,
        metadata: buildShellPolicyMetadata(policy),
      })),
      profiles: shellEditorProfiles.map((profile) => ({
        id: profile.id.trim(),
        name: profile.name.trim(),
        description: profile.description.trim() || null,
        enabled: profile.enabled,
        metadata: buildShellProfileMetadata(profile),
      })),
    };

    setShellSaveLoading(true);
    try {
      const saved = await updateRemoteShellConfig(parsed);
      setShellConfig(saved);
      setShellEditorOpen(false);
      message.success("Shell access config saved");
      void refreshGrants();
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to save shell access config",
      );
    } finally {
      setShellSaveLoading(false);
    }
  };

  const openFileAccessEditor = () => {
    setFileAccessEditorGrants(fileAccessConfig?.grant?.map(g => ({ ...g })) ?? []);
    setFileAccessEditorOpen(true);
  };

  const handleSaveFileAccessConfig = async () => {
    for (const g of fileAccessEditorGrants) {
      if (!g.grant_id.trim()) {
        message.error("Every file access policy needs a grant ID");
        return;
      }
    }
    const grantIds = new Set<string>();
    for (const g of fileAccessEditorGrants) {
      const id = g.grant_id.trim();
      if (grantIds.has(id)) {
        message.error(`Duplicate grant ID: ${id}`);
        return;
      }
      grantIds.add(id);
    }

    setFileAccessSaveLoading(true);
    try {
      const saved = await updateFileAccessConfig({
        grant: fileAccessEditorGrants.map(g => ({
          ...g,
          grant_id: g.grant_id.trim(),
          name: g.name?.trim() || undefined,
          roots: g.roots?.filter(r => r.trim()) ?? [],
          denies: g.denies?.filter(d => d.trim()) ?? [],
          write_denies: g.write_denies?.filter(d => d.trim()) ?? [],
          ops: g.ops && g.ops.length > 0 ? g.ops : undefined,
        })),
      });
      setFileAccessConfig(saved);
      setFileAccessEditorOpen(false);
      message.success("File access config saved");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to save file access config",
      );
    } finally {
      setFileAccessSaveLoading(false);
    }
  };

  const openCreateModal = () => {
    sshForm.setFieldsValue({
      label: sshKey?.label ?? "",
      grant_mode: "permanent",
    });
    setEditorOpen(true);
  };

  const presentSecretPayload = (
    payload: RemoteInvokeSshKeySecretPayload,
    mode: "created" | "downloaded" | "reset",
  ) => {
    const titleMap = {
      created: "SSH key created",
      downloaded: "Bifrost key file",
      reset: "SSH key rotated",
    } as const;
    const descriptionMap = {
      created:
        "This Bifrost key file contains the device code and private key. Copy it now and store it securely.",
      downloaded:
        "Only trusted administrators should retrieve this file. Share it only with approved callers.",
      reset:
        "A new key pair is active now. Any previous SSH grants tied to the old key should be treated as revoked.",
    } as const;

    setSecretModal({
      title: titleMap[mode],
      description: descriptionMap[mode],
      payload,
    });
  };

  const handleSubmitSshForm = async () => {
    const values = await sshForm.validateFields();
    setSshAction("create");
    try {
      const payload = await createRemoteInvokeSshKey(values);
      presentSecretPayload(payload, "created");
      message.success("SSH key created");
      setEditorOpen(false);
      await refreshSshKey({ silent: true });
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to save SSH key settings",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleFetchPrivateKey = async () => {
    setSshAction("download");
    try {
      const payload = await getRemoteInvokeSshPrivateKey();
      presentSecretPayload(payload, "downloaded");
      message.success("SSH key file loaded");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to fetch key file",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleResetSshKey = async () => {
    setSshAction("reset");
    try {
      const payload = await resetRemoteInvokeSshKey();
      presentSecretPayload(payload, "reset");
      await refreshSshKey({ silent: true });
      void refreshGrants();
      message.success("SSH key rotated");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to rotate SSH key",
      );
    } finally {
      setSshAction(null);
    }
  };

  const handleRevokeSshKey = async () => {
    setSshAction("revoke");
    try {
      await revokeRemoteInvokeSshKey();
      setSshKey(null);
      void refreshGrants();
      message.success("SSH key revoked");
    } catch (e) {
      message.error(
        e instanceof Error ? e.message : "Failed to revoke SSH key",
      );
    } finally {
      setSshAction(null);
    }
  };

  const discoverySession: DiscoverySession | null =
    status?.discovery_session ?? null;
  const pairingList = pendingPairings;
  const enabledPolicyCount =
    shellConfig?.policies.filter((policy) => policy.enabled).length ?? 0;
  const enabledProfileCount =
    shellConfig?.profiles.filter((profile) => profile.enabled).length ?? 0;
  const shellAccessMode =
    shellConfig && (shellConfig.policies.length > 0 || shellConfig.profiles.length > 0)
      ? inferShellAccessMode(
          shellConfig.profiles.map(toShellProfileEditorItem),
          shellConfig.policies.map(toShellPolicyEditorItem),
        )
      : "custom";
  const shellProfileOptions = shellEditorProfiles.map((profile) => ({
    label: `${profile.name || profile.id} (${profile.id})`,
    value: profile.id,
  }));

  return (
    <div data-testid="settings-remote-invoke-tab" style={{ paddingBottom: 20 }}>
      <Row gutter={[16, 16]}>
        <Col xs={24}>
          <Alert
            showIcon
            type="info"
            message="Remote Command Bridge"
            description="Allows authorized callers to execute read-only queries on this Bifrost instance via a relay server. Enter discovery mode and share the pair code to begin."
          />
        </Col>

        <Col xs={24} md={12} style={{ display: "flex" }}>
          <Card
            data-testid="settings-remote-invoke-status-card"
            title={
              <Space>
                <ApiOutlined />
                <span>Remote Status</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => {
                  setLoading(true);
                  void refresh().finally(() => setLoading(false));
                }}
                loading={loading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
            bodyStyle={{
              height: "100%",
              display: "flex",
              flexDirection: "column",
              gap: 16,
            }}
          >
            <div
              data-testid="settings-remote-invoke-connection-section"
              style={{
                paddingBottom: 16,
                borderBottom: "1px solid #f0f0f0",
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                Connection Status
              </Text>
              <Descriptions size="small" column={1}>
                <Descriptions.Item label="Relay Connection">
                  {status ? renderStateTag(status.state) : <Tag>Loading</Tag>}
                </Descriptions.Item>
                <Descriptions.Item label="Instance ID">
                  <Text code style={{ fontSize: 11 }}>
                    {identity?.instance_id ?? "-"}
                  </Text>
                </Descriptions.Item>
                <Descriptions.Item label="Device">
                  {identity?.device_name ?? "-"} ({identity?.platform ?? "-"})
                </Descriptions.Item>
                <Descriptions.Item label="Active Calls">
                  <Badge
                    count={status?.active_call_ids?.length ?? 0}
                    showZero
                    style={{
                      backgroundColor:
                        (status?.active_call_ids?.length ?? 0) > 0
                          ? "#52c41a"
                          : "#d9d9d9",
                    }}
                  />
                </Descriptions.Item>
              </Descriptions>
            </div>

            <div
              data-testid="settings-remote-invoke-discovery-section"
              style={{
                flex: 1,
                display: "flex",
                flexDirection: "column",
                minHeight: 180,
              }}
            >
              <Text strong style={{ display: "block", marginBottom: 12 }}>
                Discovery Mode
              </Text>
              {discoverySession ? (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 28,
                  }}
                >
                  <div style={{ textAlign: "center" }}>
                    <Title
                      level={2}
                      style={{
                        fontFamily: "monospace",
                        letterSpacing: "0.3em",
                        margin: 0,
                      }}
                    >
                      {discoverySession.pair_code}
                    </Title>
                    <Text type="secondary">
                      {countdown === "Expired" ? (
                        <Tag color="red">Expired</Tag>
                      ) : (
                        <>Expires in {countdown}</>
                      )}
                    </Text>
                  </div>
                  <Space wrap style={{ justifyContent: "center", width: "100%" }}>
                    <Button
                      icon={<CopyOutlined />}
                      onClick={handleCopyCode}
                      size="small"
                    >
                      Copy Code
                    </Button>
                    <Button
                      icon={<ReloadOutlined />}
                      onClick={handleRefreshCode}
                      loading={discoveryLoading}
                      size="small"
                    >
                      Refresh
                    </Button>
                    <Button
                      icon={<StopOutlined />}
                      onClick={handleExitDiscovery}
                      loading={discoveryLoading}
                      danger
                      size="small"
                    >
                      Exit Discovery
                    </Button>
                  </Space>
                </Space>
              ) : (
                <Space
                  direction="vertical"
                  size={12}
                  style={{
                    width: "100%",
                    flex: 1,
                    justifyContent: "flex-start",
                    paddingTop: 20,
                    textAlign: "center",
                  }}
                >
                  <DisconnectOutlined
                    style={{ fontSize: 32, color: "#bfbfbf" }}
                  />
                  <Text type="secondary">
                    Not in discovery mode. Enter discovery to generate a pair
                    code.
                  </Text>
                  <Button
                    type="primary"
                    icon={<ScanOutlined />}
                    onClick={handleEnterDiscovery}
                    loading={discoveryLoading}
                    disabled={status?.state?.toLowerCase() !== "connected"}
                  >
                    Enter Discovery Mode
                  </Button>
                  {status?.state?.toLowerCase() !== "connected" && (
                    <Text type="warning" style={{ fontSize: 12 }}>
                      Relay must be connected before entering discovery mode.
                    </Text>
                  )}
                </Space>
              )}
            </div>
          </Card>
        </Col>

        <Col xs={24} md={12} style={{ display: "flex" }}>
          <Card
            data-testid="settings-remote-invoke-ssh-card"
            title={
              <Space>
                <KeyOutlined />
                <span>SSH Key</span>
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshSshKey()}
                loading={sshLoading}
              />
            }
            size="small"
            style={{ width: "100%", height: "100%" }}
          >
            {!sshApiAvailable ? (
              <Alert
                showIcon
                type="warning"
                message="SSH key management is not available yet"
                description="This client does not expose the SSH key endpoints yet. Once the backend lands, this section will light up without further UI changes."
              />
            ) : sshKey ? (
              <Space direction="vertical" size={16} style={{ width: "100%" }}>
                <Descriptions
                  size="small"
                  column={1}
                  items={[
                    {
                      key: "label",
                      label: "Label",
                      children: sshKey.label,
                    },
                    {
                      key: "device_code",
                      label: "Device Code",
                      children: (
                        <Space size={8} wrap>
                          <Text code>{sshKey.device_code}</Text>
                          <Button
                            size="small"
                            icon={<CopyOutlined />}
                            onClick={() => {
                              copyToClipboard(sshKey.device_code);
                              message.success("Device code copied");
                            }}
                          >
                            Copy
                          </Button>
                        </Space>
                      ),
                    },
                    {
                      key: "fingerprint",
                      label: "Fingerprint",
                      children: (
                        <Text code style={{ fontSize: 11 }}>
                          {formatSshFingerprint(sshKey.ssh_key_fingerprint)}
                        </Text>
                      ),
                    },
                    {
                      key: "grant_mode",
                      label: "SSH Access",
                      children: `${formatSshGrantMode(sshKey.grant_mode)} until key revoke`,
                    },
                    {
                      key: "status",
                      label: "Status",
                      children: (
                        <Tag color={sshKey.status === "active" ? "green" : "default"}>
                          {sshKey.status}
                        </Tag>
                      ),
                    },
                    {
                      key: "last_used_at",
                      label: "Last Used",
                      children: formatTimestamp(sshKey.last_used_at),
                    },
                    {
                      key: "last_caller",
                      label: "Last Caller",
                      children: (
                        <Text type={sshKey.last_caller_info ? undefined : "secondary"}>
                          {formatCallerInfo(sshKey.last_caller_info)}
                        </Text>
                      ),
                    },
                  ]}
                />

                <Space wrap>
                  <Button
                    icon={<CopyOutlined />}
                    onClick={() => void handleFetchPrivateKey()}
                    loading={sshAction === "download"}
                  >
                    Copy Key File
                  </Button>
                  <Popconfirm
                    title="Rotate SSH key?"
                    description="This creates a new key pair, replaces the device code, and should revoke grants tied to the old key."
                    okText="Rotate"
                    cancelText="Cancel"
                    onConfirm={() => void handleResetSshKey()}
                  >
                    <Button loading={sshAction === "reset"}>
                      Reset Key
                    </Button>
                  </Popconfirm>
                  <Popconfirm
                    title="Revoke SSH key?"
                    description="SSH callers will lose access until a new key is created."
                    okText="Revoke"
                    cancelText="Cancel"
                    onConfirm={() => void handleRevokeSshKey()}
                  >
                    <Button
                      danger
                      icon={<DeleteOutlined />}
                      loading={sshAction === "revoke"}
                    >
                      Revoke Key
                    </Button>
                  </Popconfirm>
                </Space>
              </Space>
            ) : (
              <Space
                direction="vertical"
                size={12}
                style={{ width: "100%", textAlign: "center" }}
              >
                <KeyOutlined style={{ fontSize: 32, color: "#bfbfbf" }} />
                <Text type="secondary">
                  No active SSH key. Create one to provision long-lived callers without pair codes.
                </Text>
                <Button
                  type="primary"
                  onClick={openCreateModal}
                  loading={sshAction === "create"}
                >
                  Create SSH Key
                </Button>
              </Space>
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            data-testid="settings-remote-invoke-shell-card"
            title={
              <Space>
                <SafetyOutlined />
                <span>Shell Access</span>
              </Space>
            }
            extra={
              <Space>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshShellConfig()}
                  loading={shellLoading}
                />
                <Button
                  size="small"
                  icon={<EditOutlined />}
                  onClick={openShellEditor}
                  disabled={shellLoading}
                >
                  Manage Access
                </Button>
              </Space>
            }
            size="small"
          >
            <Alert
              showIcon
              type="info"
              style={{ marginBottom: 16 }}
              message="Shell execution is governed by local policies and profiles on this device"
              description="Policies decide what the caller may execute, while profiles define sandbox boundaries such as cwd, env allowlist, timeout, stdin, and interactive mode."
            />
            <Descriptions size="small" column={2} style={{ marginBottom: 16 }}>
              <Descriptions.Item label="Configuration Mode">
                <Tag color={shellAccessMode === "custom" ? "default" : "blue"}>
                  {shellAccessModeLabel(shellAccessMode)}
                </Tag>
              </Descriptions.Item>
              <Descriptions.Item label="Policy Set Version">
                {shellConfig?.version ?? "-"}
              </Descriptions.Item>
              <Descriptions.Item label="Schema Version">
                {shellConfig?.schema_version ?? "-"}
              </Descriptions.Item>
              <Descriptions.Item label="Policies">
                {shellConfig?.policies.length ?? 0} total · {enabledPolicyCount} enabled
              </Descriptions.Item>
              <Descriptions.Item label="Profiles">
                {shellConfig?.profiles.length ?? 0} total · {enabledProfileCount} enabled
              </Descriptions.Item>
            </Descriptions>

            <Row gutter={[16, 16]}>
              <Col xs={24} lg={12}>
                <Card size="small" title="Policies">
                  {!shellConfig || shellConfig.policies.length === 0 ? (
                    <Empty
                      image={Empty.PRESENTED_IMAGE_SIMPLE}
                      description="No shell policies configured"
                    />
                  ) : (
                    <List
                      size="small"
                      dataSource={shellConfig.policies}
                      renderItem={(policy) => {
                        const metadata = isRecord(policy.metadata)
                          ? policy.metadata
                          : undefined;
                        const execMode = getStringField(metadata, "exec_mode");
                        const allowedExecutables = getStringArrayField(
                          metadata,
                          "allowed_executables",
                        );
                        const allowedShellPatterns = getStringArrayField(
                          metadata,
                          "allowed_shell_patterns",
                        );

                        return (
                          <List.Item>
                            <List.Item.Meta
                              title={
                                <Space wrap>
                                  <Text strong>{policy.name}</Text>
                                  <Tag color={policy.enabled ? "green" : "default"}>
                                    {policy.enabled ? "enabled" : "disabled"}
                                  </Tag>
                                  <Tag>{policy.id}</Tag>
                                  {execMode && <Tag color="blue">{execMode}</Tag>}
                                </Space>
                              }
                              description={
                                <Space size={4} wrap>
                                  {policy.profile_id && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      profile {policy.profile_id}
                                    </Text>
                                  )}
                                  {allowedExecutables.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      · executables {allowedExecutables.length}
                                    </Text>
                                  )}
                                  {allowedShellPatterns.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      · shell regex {allowedShellPatterns.length}
                                    </Text>
                                  )}
                                </Space>
                              }
                            />
                          </List.Item>
                        );
                      }}
                    />
                  )}
                </Card>
              </Col>

              <Col xs={24} lg={12}>
                <Card size="small" title="Profiles">
                  {!shellConfig || shellConfig.profiles.length === 0 ? (
                    <Empty
                      image={Empty.PRESENTED_IMAGE_SIMPLE}
                      description="No shell profiles configured"
                    />
                  ) : (
                    <List
                      size="small"
                      dataSource={shellConfig.profiles}
                      renderItem={(profile) => {
                        const metadata = isRecord(profile.metadata)
                          ? profile.metadata
                          : undefined;
                        const cwdAllowlist = getStringArrayField(
                          metadata,
                          "cwd_allowlist",
                        );
                        const envAllowlist = getStringArrayField(
                          metadata,
                          "env_allowlist",
                        );
                        const defaultCwd = getStringField(metadata, "default_cwd");
                        const timeoutMs = getNumberField(metadata, "max_timeout_ms");

                        return (
                          <List.Item>
                            <List.Item.Meta
                              title={
                                <Space wrap>
                                  <Text strong>{profile.name}</Text>
                                  <Tag color={profile.enabled ? "green" : "default"}>
                                    {profile.enabled ? "enabled" : "disabled"}
                                  </Tag>
                                  <Tag>{profile.id}</Tag>
                                </Space>
                              }
                              description={
                                <Space size={4} wrap>
                                  {cwdAllowlist.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      cwd {cwdAllowlist.length}
                                    </Text>
                                  )}
                                  {envAllowlist.length > 0 && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      · env {envAllowlist.length}
                                    </Text>
                                  )}
                                  {defaultCwd && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      · default cwd {defaultCwd}
                                    </Text>
                                  )}
                                  {timeoutMs != null && (
                                    <Text type="secondary" style={{ fontSize: 11 }}>
                                      · timeout {timeoutMs}ms
                                    </Text>
                                  )}
                                </Space>
                              }
                            />
                          </List.Item>
                        );
                      }}
                    />
                  )}
                </Card>
              </Col>
            </Row>
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            data-testid="settings-remote-invoke-file-access-card"
            title={
              <Space>
                <FileOutlined />
                <span>File Access</span>
              </Space>
            }
            extra={
              <Space>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshFileAccessConfig()}
                  loading={fileAccessLoading}
                />
                <Button
                  size="small"
                  icon={<EditOutlined />}
                  onClick={openFileAccessEditor}
                  disabled={fileAccessLoading}
                >
                  Manage Policies
                </Button>
              </Space>
            }
            size="small"
          >
            <Alert
              showIcon
              type="info"
              style={{ marginBottom: 16 }}
              message="File access is governed by per-grant policies stored in file-access.toml"
              description="Each grant can have its own roots, deny patterns, allowed operations, and byte limits. Without explicit config, a default read-only policy applies."
            />
            <Descriptions size="small" column={2} style={{ marginBottom: 16 }}>
              <Descriptions.Item label="Grant Policies">
                {fileAccessConfig?.grant?.length ?? 0} configured
              </Descriptions.Item>
            </Descriptions>
            {!fileAccessConfig || (fileAccessConfig.grant?.length ?? 0) === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No per-grant file access policies configured. Default read-only policy applies."
              />
            ) : (
              <List
                size="small"
                dataSource={fileAccessConfig.grant}
                renderItem={(policy) => {
                  const readOps = (policy.ops ?? []).filter(op => !["write","edit","mkdir","move","delete","apply_patch"].includes(op));
                  const writeOps = (policy.ops ?? []).filter(op => ["write","edit","mkdir","move","delete","apply_patch"].includes(op));
                  return (
                    <List.Item>
                      <List.Item.Meta
                        title={
                          <Space wrap>
                            <Text strong>{policy.name || policy.grant_id}</Text>
                            <Tag>{policy.grant_id}</Tag>
                            {writeOps.length > 0 ? (
                              <Tag color="orange">read+write</Tag>
                            ) : (
                              <Tag color="blue">read-only</Tag>
                            )}
                          </Space>
                        }
                        description={
                          <Space size={4} wrap>
                            {(policy.roots?.length ?? 0) > 0 && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                roots: {policy.roots!.join(", ")}
                              </Text>
                            )}
                            {readOps.length > 0 && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                · read ops {readOps.length}
                              </Text>
                            )}
                            {writeOps.length > 0 && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                · write ops {writeOps.length}
                              </Text>
                            )}
                            {(policy.denies?.length ?? 0) > 0 && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                · denies {policy.denies!.length}
                              </Text>
                            )}
                          </Space>
                        }
                      />
                    </List.Item>
                  );
                }}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <span>Pending Pairing Requests</span>
                <Badge count={pairingList.length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void storeFetchPairings()}
              />
            }
            size="small"
          >
            {pairingList.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No pending pairing requests"
              />
            ) : (
              <List
                dataSource={pairingList}
                renderItem={(p) => (
                  <List.Item
                    actions={[
                      <Button
                        key="review"
                        type="primary"
                        size="small"
                        icon={<EyeOutlined />}
                        onClick={() => setReviewPairing(p)}
                      >
                        Review
                      </Button>,
                    ]}
                  >
                    <List.Item.Meta
                      avatar={
                        <Tooltip title={p.caller_info.fingerprint}>
                          <Tag color="blue" style={{ fontFamily: "monospace" }}>
                            {formatFingerprint(p.caller_info.fingerprint)}
                          </Tag>
                        </Tooltip>
                      }
                      title={
                        <Space>
                          <Text>
                            {p.caller_info.display_name || "Unknown Caller"}
                          </Text>
                          {p.caller_info.source_ip && (
                            <Text type="secondary" style={{ fontSize: 12 }}>
                              from {p.caller_info.source_ip}
                            </Text>
                          )}
                        </Space>
                      }
                      description={
                        <Space size={4}>
                          <Tag>
                            {p.command_summary.command_preview || p.command.command}
                          </Tag>
                          {p.caller_info.platform && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              {p.caller_info.platform}
                            </Text>
                          )}
                        </Space>
                      }
                    />
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <SafetyOutlined />
                <span>Grants</span>
                <Badge count={grants.filter((g) => g.status === "active").length} />
              </Space>
            }
            extra={
              <Button
                size="small"
                icon={<ReloadOutlined />}
                onClick={() => void refreshGrants()}
              />
            }
            size="small"
          >
            {grants.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No active grants"
              />
            ) : (
              <List
                dataSource={grants}
                size="small"
                pagination={{ pageSize: 10, size: "small", hideOnSinglePage: true }}
                renderItem={(g) => (
                  <List.Item
                    actions={g.status === "removed" ? [] : [
                      <Button
                        key="edit"
                        size="small"
                        icon={<EditOutlined />}
                        onClick={() => openGrantEditor(g)}
                      >
                        Edit Access
                      </Button>,
                      <Popconfirm
                        key="revoke"
                        title="Revoke this grant?"
                        description="The caller will need to pair again."
                        onConfirm={() => void handleRevokeGrant(g.grant_id)}
                        okText="Revoke"
                        cancelText="Cancel"
                      >
                        <Button
                          danger
                          size="small"
                          icon={<DeleteOutlined />}
                        >
                          Revoke
                        </Button>
                      </Popconfirm>,
                    ]}
                  >
                    <List.Item.Meta
                      title={
                        <Space>
                          <Text>{g.caller_display_name || formatFingerprint(g.caller_fingerprint)}</Text>
                          <Tag color={g.status === "active" ? "green" : "default"}>
                            {g.status}
                          </Tag>
                          <Tag>{g.grant_mode}</Tag>
                          <Tag color={g.grant_scope === "remote_query" ? "default" : "purple"}>
                            {g.grant_scope}
                          </Tag>
                          {g.file_access && g.file_access !== "none" && (
                            <Tag color="blue">
                              {g.file_access === "read_write" ? "File: R/W" : "File: Read"}
                            </Tag>
                          )}
                          {g.shell_policy_set_version_snapshot != null && (
                            <Tag color="blue">
                              shell set v{g.shell_policy_set_version_snapshot}
                            </Tag>
                          )}
                        </Space>
                      }
                      description={
                        <Space size={4} wrap>
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            Used {g.use_count}x
                          </Text>
                          {g.last_used_at && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · last active {new Date(g.last_used_at).toLocaleString()}
                            </Text>
                          )}
                          {g.expires_at != null && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · expires {new Date(g.expires_at).toLocaleDateString()}
                            </Text>
                          )}
                          <Tooltip title={g.caller_fingerprint}>
                            <Text type="secondary" style={{ fontSize: 10, fontFamily: "monospace" }}>
                              {formatFingerprint(g.caller_fingerprint)}
                            </Text>
                          </Tooltip>
                          {g.stdin_allowed != null && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · stdin {g.stdin_allowed ? "on" : "off"}
                            </Text>
                          )}
                          {g.interactive_allowed != null && (
                            <Text type="secondary" style={{ fontSize: 11 }}>
                              · interactive {g.interactive_allowed ? "on" : "off"}
                            </Text>
                          )}
                        </Space>
                      }
                    />
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        <Col xs={24}>
          <Card
            title={
              <Space>
                <HistoryOutlined />
                <span>Recent Calls</span>
                <Badge count={calls.length} />
              </Space>
            }
            extra={
              <Space size={4}>
                <Popconfirm
                  title="Clear recent calls?"
                  okText="Clear"
                  okButtonProps={{ danger: true }}
                  onConfirm={() => void handleClearCalls()}
                >
                  <Button
                    size="small"
                    danger
                    icon={<DeleteOutlined />}
                    disabled={calls.length === 0}
                  />
                </Popconfirm>
                <Button
                  size="small"
                  icon={<ReloadOutlined />}
                  onClick={() => void refreshCalls()}
                />
              </Space>
            }
            size="small"
          >
            {calls.length === 0 ? (
              <Empty
                image={Empty.PRESENTED_IMAGE_SIMPLE}
                description="No recent calls"
              />
            ) : (
              <List
                dataSource={calls}
                size="small"
                pagination={{ pageSize: 10, size: "small", hideOnSinglePage: true }}
                renderItem={(c) => {
                  const argsPreviewSource = getCallArgsPreviewSource(c);
                  const argsPreview = formatArgsPreview(argsPreviewSource);
                  const commandPreview = getCallCommandPreview(c);
                  const bytesLabel = formatBytes(c.bytes_out);
                  return (
                      <List.Item
                        style={recentCallItemStyle}
                        onClick={() => void handleOpenCallDetail(c)}
                      >
                        <div style={recentCallRowStyle}>
                          <div style={{ minWidth: 0 }}>
                            <div style={recentCallPrimaryStyle}>
                              <Tooltip title={commandPreview}>
                                <Text
                                  code
                                  style={{
                                    ...recentCallTextEllipsisStyle,
                                    fontSize: 11,
                                  }}
                                >
                                  {commandPreview}
                                </Text>
                              </Tooltip>
                              {argsPreview && (
                                <Tooltip title={argsPreviewSource}>
                                  <Text
                                    type="secondary"
                                    style={{
                                      ...recentCallTextEllipsisStyle,
                                      flex: "0 1 260px",
                                      fontSize: 11,
                                    }}
                                  >
                                    {argsPreview}
                                  </Text>
                                </Tooltip>
                              )}
                            </div>
                            <Space size={4} style={{ marginTop: 4 }}>
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                {new Date(c.started_at).toLocaleString()}
                              </Text>
                              {c.duration_ms != null && (
                                <Text type="secondary" style={{ fontSize: 11 }}>
                                  · {c.duration_ms}ms
                                </Text>
                              )}
                              {bytesLabel && (
                                <Text type="secondary" style={{ fontSize: 11 }}>
                                  · ↓ {bytesLabel}
                                </Text>
                              )}
                            </Space>
                          </div>

                          <div style={recentCallMetaStyle}>
                            <Tag color={getCallStatusColor(c)}>
                              {c.status}
                              {c.exit_code !== undefined && c.exit_code !== null
                                ? ` (${c.exit_code})`
                                : ""}
                            </Tag>
                            {c.caller_display_name && (
                              <Tooltip title={c.caller_display_name}>
                                <Text
                                  type="secondary"
                                  style={{
                                    ...recentCallTextEllipsisStyle,
                                    maxWidth: 220,
                                    fontSize: 11,
                                  }}
                                >
                                  by {c.caller_display_name}
                                </Text>
                              </Tooltip>
                            )}
                            {c.policy_id && (
                              <Tooltip title={c.policy_id}>
                                <Tag
                                  color="purple"
                                  style={{
                                    maxWidth: 120,
                                    overflow: "hidden",
                                    textOverflow: "ellipsis",
                                    whiteSpace: "nowrap",
                                  }}
                                >
                                  {c.policy_id}
                                </Tag>
                              </Tooltip>
                            )}
                            {c.exec_mode && (
                              <Tooltip title={c.exec_mode}>
                                <Tag
                                  color="blue"
                                  style={{
                                    maxWidth: 120,
                                    overflow: "hidden",
                                    textOverflow: "ellipsis",
                                    whiteSpace: "nowrap",
                                  }}
                                >
                                  {c.exec_mode}
                                </Tag>
                              </Tooltip>
                            )}
                          </div>

                          <Tooltip title="View call detail">
                            <Button
                              size="small"
                              icon={<EyeOutlined />}
                              onClick={(event) => {
                                event.stopPropagation();
                                void handleOpenCallDetail(c);
                              }}
                            />
                          </Tooltip>
                        </div>
                      </List.Item>
                  );
                }}
              />
            )}
          </Card>
        </Col>
      </Row>
      <PairingRequestModal
        visible={reviewPairing !== null}
        pairing={reviewPairing}
        shellConfig={shellConfig}
        onClose={() => {
          setReviewPairing(null);
          void storeFetchPairings();
        }}
      />
      <Modal
        open={callDetailOpen}
        title="Call Detail"
        footer={null}
        width={820}
        onCancel={() => {
          setCallDetailOpen(false);
          setSelectedCall(null);
        }}
        destroyOnClose
      >
        <Spin spinning={callDetailLoading}>
          {selectedCall && (
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Descriptions
                size="small"
                column={2}
                bordered
                items={[
                  {
                    key: "call_id",
                    label: "Call ID",
                    span: 2,
                    children: <Text code copyable>{selectedCall.call_id}</Text>,
                  },
                  {
                    key: "status",
                    label: "Status",
                    children: (
                      <Tag color={getCallStatusColor(selectedCall)}>
                        {selectedCall.status}
                        {selectedCall.exit_code !== undefined &&
                        selectedCall.exit_code !== null
                          ? ` (${selectedCall.exit_code})`
                          : ""}
                      </Tag>
                    ),
                  },
                  {
                    key: "started_at",
                    label: "Started",
                    children: new Date(selectedCall.started_at).toLocaleString(),
                  },
                  {
                    key: "duration",
                    label: "Duration",
                    children:
                      selectedCall.duration_ms != null
                        ? `${selectedCall.duration_ms}ms`
                        : "-",
                  },
                  {
                    key: "bytes_out",
                    label: "Bytes Out",
                    children: formatBytes(selectedCall.bytes_out) ?? "-",
                  },
                  {
                    key: "grant_id",
                    label: "Grant",
                    span: 2,
                    children: <Text code>{selectedCall.grant_id}</Text>,
                  },
                  {
                    key: "client",
                    label: "Client",
                    span: 2,
                    children: <Text code>{selectedCall.client_instance_id}</Text>,
                  },
                  {
                    key: "caller",
                    label: "Caller",
                    span: 2,
                    children:
                      selectedCall.caller_display_name ||
                      formatFingerprint(selectedCall.caller_fingerprint),
                  },
                  {
                    key: "policy",
                    label: "Policy",
                    children: selectedCall.policy_id ? (
                      <Tag color="purple">{selectedCall.policy_id}</Tag>
                    ) : (
                      "-"
                    ),
                  },
                  {
                    key: "exec_mode",
                    label: "Exec Mode",
                    children: selectedCall.exec_mode ? (
                      <Tag color="blue">{selectedCall.exec_mode}</Tag>
                    ) : (
                      "-"
                    ),
                  },
                ]}
              />
              <div>
                <Text strong>Command</Text>
                {renderCallDetailBlock(getCallCommandPreview(selectedCall))}
              </div>
              <div>
                <Text strong>Arguments</Text>
                {renderCallDetailBlock(getCallArgsPreviewSource(selectedCall))}
              </div>
              {selectedCall.command_detail && (
                <div>
                  <Text strong>Command Detail</Text>
                  {renderCallDetailBlock(selectedCall.command_detail)}
                </div>
              )}
              <div>
                <Text strong>Raw Command</Text>
                {renderCallDetailBlock(selectedCall.command)}
              </div>
            </Space>
          )}
        </Spin>
      </Modal>
      <Modal
        open={grantEditorOpen}
        title="Edit Grant Access"
        okText="Save"
        cancelText="Cancel"
        onCancel={() => {
          setGrantEditorOpen(false);
          setEditingGrant(null);
        }}
        onOk={() => void handleSaveGrant()}
        confirmLoading={grantSaveLoading}
        destroyOnClose
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Descriptions
            size="small"
            column={1}
            bordered
            items={[
              {
                key: "grant",
                label: "Grant",
                children: <Text code>{editingGrant?.grant_id ?? "-"}</Text>,
              },
              {
                key: "caller",
                label: "Caller",
                children:
                  editingGrant?.caller_display_name ||
                  formatFingerprint(editingGrant?.caller_fingerprint ?? ""),
              },
            ]}
          />
          <Radio.Group
            value={grantEditorPreset}
            onChange={(e) => setGrantEditorPreset(e.target.value)}
            style={{ width: "100%" }}
          >
            <Space direction="vertical" style={{ width: "100%" }} size={4}>
              <Radio value="full" disabled={enabledShellPolicies.length === 0}>
                <Space>
                  <ThunderboltOutlined />
                  <span><b>Full trust</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Run any allowed command, read &amp; write files, open interactive terminals
                  </Text>
                </Space>
              </Radio>
              <Radio value="shell_only" disabled={enabledShellPolicies.length === 0}>
                <Space>
                  <CodeOutlined />
                  <span><b>Run commands &amp; read/write files</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Can execute allowed commands, edit files, and inspect traffic. No interactive shells.
                  </Text>
                </Space>
              </Radio>
              <Radio value="file_only">
                <Space>
                  <FileOutlined />
                  <span><b>Files only</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Can read &amp; write files and inspect traffic. Cannot run any shell commands.
                  </Text>
                </Space>
              </Radio>
              <Radio value="query">
                <Space>
                  <SearchOutlined />
                  <span><b>Read-only watch</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Can see status and traffic. Cannot run commands and cannot touch files.
                  </Text>
                </Space>
              </Radio>
              <Radio value="custom">
                <Space>
                  <SettingOutlined />
                  <span><b>Custom</b></span>
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    — Pick shell + file + terminal access individually
                  </Text>
                </Space>
              </Radio>
            </Space>
          </Radio.Group>
          <Alert
            showIcon
            type={
              grantEditorPreset === "full"
                ? "warning"
                : grantEditorPreset === "query"
                ? "success"
                : "info"
            }
            message="Preview — what this caller will be able to do"
            description={
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                {(() => {
                  const bullets: string[] = [];
                  const shellAllowed =
                    grantEditorPreset === "full" ||
                    grantEditorPreset === "shell_only" ||
                    (grantEditorPreset === "custom" && grantEditorAccessMode !== "query");
                  const fileScope =
                    grantEditorPreset === "full" || grantEditorPreset === "shell_only"
                      ? "read_write"
                      : grantEditorPreset === "file_only"
                      ? "read_write"
                      : grantEditorPreset === "query"
                      ? "none"
                      : grantEditorFileAccess;
                  const interactive =
                    grantEditorPreset === "full" ||
                    (grantEditorPreset === "custom" && grantEditorInteractiveAllowed);
                  const stdin =
                    grantEditorPreset === "full" ||
                    (grantEditorPreset === "custom" && grantEditorStdinAllowed);

                  if (shellAllowed) {
                    if (grantEditorPreset === "custom" && grantEditorAccessMode === "selected") {
                      const names = enabledShellPolicies
                        .filter((p) => grantEditorSelectedPolicies.includes(p.id))
                        .map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? "Run commands from: " + names.join(", ")
                          : "Run commands from: (none selected — pick groups below)"
                      );
                    } else {
                      const names = enabledShellPolicies.map((p) => p.name || p.id);
                      bullets.push(
                        names.length
                          ? "Run commands from any of: " + names.join(", ")
                          : "No command groups are enabled yet; add one in Shell Access settings"
                      );
                    }
                    bullets.push(
                      interactive
                        ? "Can open an interactive terminal"
                        : stdin
                        ? "Can send stdin (one-shot input) but no interactive terminal"
                        : "No stdin, no interactive terminal"
                    );
                  } else {
                    bullets.push("Cannot run any shell commands");
                  }

                  if (fileScope === "read_write") {
                    bullets.push("Can read and write files on this device");
                  } else if (fileScope === "read") {
                    bullets.push("Can read files but cannot modify them");
                  } else {
                    bullets.push("No file access");
                  }

                  bullets.push("Can see connection status and inspect traffic records");

                  return bullets.map((b, i) => <li key={i}>{b}</li>);
                })()}
              </ul>
            }
            style={{ marginTop: 4 }}
          />
          {grantEditorPreset === "custom" && (
            <div
              style={{
                padding: 12,
                background: "var(--ant-color-bg-container-disabled, #fafafa)",
                borderRadius: 8,
              }}
            >
              <Space direction="vertical" size={8} style={{ width: "100%" }}>
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>Shell Access</Text>
                  <Select
                    value={grantEditorAccessMode}
                    onChange={setGrantEditorAccessMode}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "query", label: "Just watch traffic (no commands)" },
                      {
                        value: "selected",
                        label: "Run only commands from selected groups",
                        disabled: enabledShellPolicies.length === 0,
                      },
                      {
                        value: "all",
                        label: "Run commands from any enabled group",
                        disabled: enabledShellPolicies.length === 0,
                      },
                    ]}
                  />
                </div>
                {grantEditorAccessMode === "selected" && (
                  <Select
                    mode="multiple"
                    placeholder="Choose shell policies"
                    value={grantEditorSelectedPolicies}
                    onChange={setGrantEditorSelectedPolicies}
                    style={{ width: "100%" }}
                    options={enabledShellPolicies.map((policy) => ({
                      value: policy.id,
                      label: `${policy.name} (${policy.id})`,
                    }))}
                  />
                )}
                {grantEditorAccessMode !== "query" && (
                  <Space>
                    <Text type="secondary">Allow stdin</Text>
                    <Switch
                      checked={grantEditorStdinAllowed}
                      onChange={setGrantEditorStdinAllowed}
                    />
                    <Text type="secondary">Interactive</Text>
                    <Switch
                      checked={grantEditorInteractiveAllowed}
                      onChange={setGrantEditorInteractiveAllowed}
                    />
                  </Space>
                )}
                <div>
                  <Text type="secondary" style={{ fontSize: 12 }}>File Access</Text>
                  <Select
                    value={grantEditorFileAccess}
                    onChange={setGrantEditorFileAccess}
                    style={{ width: "100%", marginTop: 4 }}
                    options={[
                      { value: "none", label: "No file access (cannot read or write files)" },
                      { value: "read", label: "Read-only (can open files, cannot modify)" },
                      { value: "read_write", label: "Read & write (can modify files)" },
                    ]}
                  />
                </div>
              </Space>
            </div>
          )}
        </Space>
      </Modal>
      <Modal
        open={shellEditorOpen}
        title="Shell access — what remote agents can run on this device"
        okText="Save"
        cancelText="Cancel"
        onCancel={() => setShellEditorOpen(false)}
        onOk={() => void handleSaveShellConfig()}
        confirmLoading={shellSaveLoading}
        width={960}
        destroyOnClose
      >
        <Alert
          showIcon
          type="warning"
          style={{ marginBottom: 16 }}
          message="Changing shell access rules may require callers to re-authorize"
          description="Policy set version is bound into shell grants. If you tighten or rename policies, existing shell authorizations may stop working until the caller reconnects."
        />
        <Space direction="vertical" size={20} style={{ width: "100%" }}>
          <Card size="small" title="Access Mode">
            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              <Text type="secondary">
                Pick a preset for the common cases, or choose <b>Custom Rules</b>
                to define exactly which commands and folders are allowed.
              </Text>
              <Radio.Group
                value={shellEditorMode}
                onChange={(e) => applyShellAccessMode(e.target.value)}
                optionType="button"
                buttonStyle="solid"
                style={{ display: "flex", flexWrap: "wrap", gap: 8 }}
              >
                <Radio.Button value="sandbox">Safe Sandbox</Radio.Button>
                <Radio.Button value="full-access">Trusted (Full Access)</Radio.Button>
                <Radio.Button value="custom">Custom Rules</Radio.Button>
              </Radio.Group>
              {shellEditorMode === "custom" ? (
                <Alert
                  showIcon
                  type="info"
                  message="Custom rules"
                  description={
                    <div>
                      <div style={{ marginBottom: 4 }}>
                        Define exactly which commands an agent can run and where.
                      </div>
                      <ul style={{ margin: 0, paddingLeft: 18 }}>
                        <li>Create one or more <b>command groups</b> below.</li>
                        <li>Each group is a set of allowed commands + an execution environment.</li>
                        <li>Bind a group to a caller when you approve a pairing request.</li>
                      </ul>
                    </div>
                  }
                />
              ) : (
                <Alert
                  showIcon
                  type={shellEditorMode === "full-access" ? "warning" : "success"}
                  message={shellPresetDefinition(shellEditorMode).label}
                  description={
                    <div>
                      <div style={{ marginBottom: 6 }}>
                        {shellPresetDefinition(shellEditorMode).description}
                      </div>
                      <ul style={{ margin: 0, paddingLeft: 18 }}>
                        {shellPresetDefinition(shellEditorMode).capabilities.map((cap, idx) => (
                          <li
                            key={idx}
                            style={{
                              color: cap.allowed ? "var(--ant-color-success, #52c41a)" : "var(--ant-color-text-tertiary, #999)",
                            }}
                          >
                            <span style={{ fontWeight: 600, marginRight: 6 }}>
                              {cap.allowed ? "✓" : "✗"}
                            </span>
                            <span style={{ color: "var(--ant-color-text, inherit)" }}>{cap.text}</span>
                          </li>
                        ))}
                      </ul>
                    </div>
                  }
                />
              )}
            </Space>
          </Card>

          {shellEditorMode === "custom" ? (
          <>
          <div>
            <Space
              align="center"
              style={{ width: "100%", justifyContent: "space-between" }}
            >
              <Title level={5} style={{ margin: 0 }}>
                Execution environments <Text type="secondary" style={{ fontWeight: 400, fontSize: 13 }}>(profiles)</Text>
              </Title>
              <Button
                icon={<PlusOutlined />}
                onClick={() => {
                  const nextId = nextShellItemId(
                    "profile",
                    shellEditorProfiles.map((profile) => profile.id),
                  );
                  const nextIndex = shellEditorProfiles.length + 1;
                  setShellEditorProfiles((prev) => [
                    ...prev,
                    {
                      id: nextId,
                      name: `Profile ${nextIndex}`,
                      description: "",
                      enabled: true,
                      cwd_allowlist: [],
                      env_allowlist: [],
                      default_cwd: "",
                      max_timeout_ms: null,
                      stdin_allowed: false,
                      interactive_allowed: false,
                      extra_metadata: {},
                    },
                  ]);
                }}
              >
                Add execution environment
              </Button>
            </Space>
            <Text type="secondary">
              <b>Execution environments</b> — how the agent's commands will run:
              which folders, which environment variables it can see, timeout, and
              whether stdin / interactive terminals are allowed.
            </Text>
            <Divider style={{ margin: "12px 0" }} />

            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              {shellEditorProfiles.length === 0 ? (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description="No profiles yet"
                />
              ) : (
                shellEditorProfiles.map((profile, index) => (
                  <Card
                    key={`profile-editor-${profile.id}-${index}`}
                    size="small"
                    title={profile.name || profile.id || `Profile ${index + 1}`}
                    extra={
                      <Button
                        size="small"
                        danger
                        icon={<DeleteOutlined />}
                        onClick={() =>
                          setShellEditorProfiles((prev) =>
                            prev.filter((_, current) => current !== index),
                          )
                        }
                      >
                        Remove
                      </Button>
                    }
                  >
                    <Row gutter={[12, 12]}>
                      <Col xs={24} md={10}>
                        <Text type="secondary">Profile name</Text>
                        <Input
                          value={profile.name}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, name: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={10}>
                        <Text type="secondary">Profile ID</Text>
                        <Input
                          value={profile.id}
                          readOnly
                        />
                      </Col>
                      <Col xs={24} md={4}>
                        <Text type="secondary">Enabled</Text>
                        <div>
                          <Switch
                            checked={profile.enabled}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, enabled: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={24}>
                        <Text type="secondary">Description</Text>
                        <Input
                          value={profile.description}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, description: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed working directories</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={profile.cwd_allowlist}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, cwd_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Add allowed cwd prefixes"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed environment keys</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={profile.env_allowlist}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, env_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Add allowed env keys"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Default working directory</Text>
                        <Input
                          value={profile.default_cwd}
                          onChange={(e) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, default_cwd: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Max timeout (ms)</Text>
                        <InputNumber
                          style={{ width: "100%" }}
                          min={1}
                          value={profile.max_timeout_ms ?? undefined}
                          onChange={(value) =>
                            setShellEditorProfiles((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? {
                                      ...item,
                                      max_timeout_ms:
                                        typeof value === "number"
                                          ? value
                                          : null,
                                    }
                                  : item,
                              ),
                            )
                          }
                          placeholder="Optional"
                        />
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">Allow stdin</Text>
                        <div>
                          <Switch
                            checked={profile.stdin_allowed}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, stdin_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">Allow interactive</Text>
                        <div>
                          <Switch
                            checked={profile.interactive_allowed}
                            onChange={(checked) =>
                              setShellEditorProfiles((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, interactive_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                    </Row>
                  </Card>
                ))
              )}
            </Space>
          </div>
          <div>
            <Space
              align="center"
              style={{ width: "100%", justifyContent: "space-between" }}
            >
              <Title level={5} style={{ margin: 0 }}>
                Command groups <Text type="secondary" style={{ fontWeight: 400, fontSize: 13 }}>(policies)</Text>
              </Title>
              <Button
                icon={<PlusOutlined />}
                onClick={() => {
                  const nextId = nextShellItemId(
                    "policy",
                    shellEditorPolicies.map((policy) => policy.id),
                  );
                  const nextIndex = shellEditorPolicies.length + 1;
                  setShellEditorPolicies((prev) => [
                    ...prev,
                    {
                      id: nextId,
                      name: `Policy ${nextIndex}`,
                      description: "",
                      enabled: true,
                      profile_id: shellEditorProfiles[0]?.id,
                      exec_mode: "argv_exec",
                      allowed_executables: [],
                      allowed_shell_patterns: [],
                      cwd_allowlist: [],
                      env_allowlist: [],
                      default_cwd: "",
                      max_timeout_ms: null,
                      stdin_allowed: false,
                      interactive_allowed: false,
                      extra_metadata: {},
                    },
                  ]);
                }}
              >
                Add command group
              </Button>
            </Space>
            <Text type="secondary">
              Policies define what callers may execute and which profile applies.
            </Text>
            <Divider style={{ margin: "12px 0" }} />

            <Space direction="vertical" size={12} style={{ width: "100%" }}>
              {shellEditorPolicies.length === 0 ? (
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description="No policies yet"
                />
              ) : (
                shellEditorPolicies.map((policy, index) => (
                  <Card
                    key={`policy-editor-${policy.id}-${index}`}
                    size="small"
                    title={policy.name || policy.id || `Policy ${index + 1}`}
                    extra={
                      <Button
                        size="small"
                        danger
                        icon={<DeleteOutlined />}
                        onClick={() =>
                          setShellEditorPolicies((prev) =>
                            prev.filter((_, current) => current !== index),
                          )
                        }
                      >
                        Remove
                      </Button>
                    }
                  >
                    <Row gutter={[12, 12]}>
                      <Col xs={24} md={8}>
                        <Text type="secondary">Policy name</Text>
                        <Input
                          value={policy.name}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, name: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={8}>
                        <Text type="secondary">Policy ID</Text>
                        <Input
                          value={policy.id}
                          readOnly
                        />
                      </Col>
                      <Col xs={24} md={4}>
                        <Text type="secondary">Enabled</Text>
                        <div>
                          <Switch
                            checked={policy.enabled}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, enabled: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={24} md={4}>
                        <Text type="secondary">Exec mode</Text>
                        <Select
                          style={{ width: "100%" }}
                          value={policy.exec_mode}
                          onChange={(value: ShellExecMode) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, exec_mode: value }
                                  : item,
                              ),
                            )
                          }
                        >
                          <Option value="argv_exec">argv_exec</Option>
                          <Option value="shell_text">shell_text</Option>
                        </Select>
                      </Col>
                      <Col xs={24}>
                        <Text type="secondary">Description</Text>
                        <Input
                          value={policy.description}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, description: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Sandbox profile</Text>
                        <Select
                          allowClear
                          style={{ width: "100%" }}
                          value={policy.profile_id}
                          options={shellProfileOptions}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, profile_id: value }
                                  : item,
                              ),
                            )
                          }
                          placeholder="Choose a profile"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed executables</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.allowed_executables}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, allowed_executables: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Add executable paths"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed shell patterns</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.allowed_shell_patterns}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, allowed_shell_patterns: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Add regex patterns"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed working directories</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.cwd_allowlist}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, cwd_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Optional extra cwd restrictions"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Allowed environment keys</Text>
                        <Select
                          mode="tags"
                          style={{ width: "100%" }}
                          value={policy.env_allowlist}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, env_allowlist: value }
                                  : item,
                              ),
                            )
                          }
                          tokenSeparators={[","]}
                          placeholder="Optional env restrictions"
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Default working directory</Text>
                        <Input
                          value={policy.default_cwd}
                          onChange={(e) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? { ...item, default_cwd: e.target.value }
                                  : item,
                              ),
                            )
                          }
                        />
                      </Col>
                      <Col xs={24} md={12}>
                        <Text type="secondary">Max timeout (ms)</Text>
                        <InputNumber
                          style={{ width: "100%" }}
                          min={1}
                          value={policy.max_timeout_ms ?? undefined}
                          onChange={(value) =>
                            setShellEditorPolicies((prev) =>
                              prev.map((item, current) =>
                                current === index
                                  ? {
                                      ...item,
                                      max_timeout_ms:
                                        typeof value === "number"
                                          ? value
                                          : null,
                                    }
                                  : item,
                              ),
                            )
                          }
                          placeholder="Optional"
                        />
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">Allow stdin</Text>
                        <div>
                          <Switch
                            checked={policy.stdin_allowed}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, stdin_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                      <Col xs={12} md={6}>
                        <Text type="secondary">Allow interactive</Text>
                        <div>
                          <Switch
                            checked={policy.interactive_allowed}
                            onChange={(checked) =>
                              setShellEditorPolicies((prev) =>
                                prev.map((item, current) =>
                                  current === index
                                    ? { ...item, interactive_allowed: checked }
                                    : item,
                                ),
                              )
                            }
                          />
                        </div>
                      </Col>
                    </Row>
                  </Card>
                ))
              )}
            </Space>
          </div>
          </>
          ) : (
            <Card size="small" title="Preset Preview">
              <Space direction="vertical" size={16} style={{ width: "100%" }}>
                <Descriptions size="small" column={1} bordered>
                  <Descriptions.Item label="Mode">
                    {shellPresetDefinition(shellEditorMode).label}
                  </Descriptions.Item>
                  <Descriptions.Item label="Policies">
                    {shellEditorPolicies.map((policy) => (
                      <Tag key={policy.id} color="blue" style={{ marginBottom: 4 }}>
                        {policy.name} ({policy.id})
                      </Tag>
                    ))}
                  </Descriptions.Item>
                  <Descriptions.Item label="Profiles">
                    {shellEditorProfiles.length > 0 ? (
                      shellEditorProfiles.map((profile) => (
                        <Tag key={profile.id} color="green" style={{ marginBottom: 4 }}>
                          {profile.name} ({profile.id})
                        </Tag>
                      ))
                    ) : (
                      <Text type="secondary">No extra profile. Policy metadata applies directly.</Text>
                    )}
                  </Descriptions.Item>
                </Descriptions>
                <Alert
                  showIcon
                  type="info"
                  message="This preset still saves into the standard shell policy store"
                  description="You can switch back to <b>Custom Rules</b> any time to fine-tune allowed commands, folders, timeouts, stdin, or interactive terminals."
                />
              </Space>
            </Card>
          )}
        </Space>
      </Modal>
      <Modal
        open={fileAccessEditorOpen}
        title="Manage File Access Policies"
        okText="Save"
        cancelText="Cancel"
        onCancel={() => setFileAccessEditorOpen(false)}
        onOk={() => void handleSaveFileAccessConfig()}
        confirmLoading={fileAccessSaveLoading}
        width={960}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message="Per-grant file access policies"
          description="Each entry maps a grant_id to its file access rules. Grants without an entry use a default read-only policy rooted at the caller's working directory."
        />
        <Space direction="vertical" size={16} style={{ width: "100%" }}>
          <Space align="center" style={{ width: "100%", justifyContent: "space-between" }}>
            <Title level={5} style={{ margin: 0 }}>Grant Policies</Title>
            <Button
              icon={<PlusOutlined />}
              onClick={() => {
                setFileAccessEditorGrants(prev => [
                  ...prev,
                  {
                    grant_id: "",
                    name: "",
                    roots: [],
                    denies: ["**/.git/**", "**/target/**", "**/*.key", "**/*.pem"],
                    write_denies: [],
                    ops: [...FILE_READ_OPS],
                    respect_gitignore: true,
                    allow_overwrite: true,
                    allow_recursive_delete: false,
                  },
                ]);
              }}
            >
              Add Policy
            </Button>
          </Space>

          {fileAccessEditorGrants.length === 0 ? (
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description="No per-grant policies. Add one to customize file access for specific grants."
            />
          ) : (
            fileAccessEditorGrants.map((grant, index) => (
              <Card
                key={`fa-grant-${index}`}
                size="small"
                title={grant.name || grant.grant_id || `Policy ${index + 1}`}
                extra={
                  <Button
                    size="small"
                    danger
                    icon={<DeleteOutlined />}
                    onClick={() => {
                      setFileAccessEditorGrants(prev => prev.filter((_, i) => i !== index));
                    }}
                  />
                }
              >
                <Space direction="vertical" size={12} style={{ width: "100%" }}>
                  <Row gutter={12}>
                    <Col span={12}>
                      <Text type="secondary" style={{ fontSize: 12 }}>Grant ID *</Text>
                      <Input
                        value={grant.grant_id}
                        placeholder="e.g. g-abc123"
                        onChange={e => {
                          const val = e.target.value;
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, grant_id: val } : g));
                        }}
                      />
                    </Col>
                    <Col span={12}>
                      <Text type="secondary" style={{ fontSize: 12 }}>Name</Text>
                      <Input
                        value={grant.name ?? ""}
                        placeholder="Human-readable label"
                        onChange={e => {
                          const val = e.target.value;
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, name: val } : g));
                        }}
                      />
                    </Col>
                  </Row>

                  <div>
                    <Text type="secondary" style={{ fontSize: 12 }}>Allowed Roots (one per line)</Text>
                    <TextArea
                      autoSize={{ minRows: 2, maxRows: 5 }}
                      value={(grant.roots ?? []).join("\n")}
                      placeholder="/Users/eden/work/project"
                      onChange={e => {
                        const roots = e.target.value.split("\n");
                        setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, roots } : g));
                      }}
                    />
                  </div>

                  <div>
                    <Text type="secondary" style={{ fontSize: 12 }}>Allowed Operations</Text>
                    <div style={{ marginTop: 4 }}>
                      <Select
                        mode="multiple"
                        style={{ width: "100%" }}
                        value={grant.ops ?? []}
                        onChange={(ops: FileOp[]) => {
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, ops } : g));
                        }}
                        options={ALL_FILE_OPS.map(op => ({ value: op, label: op }))}
                      />
                      <Space style={{ marginTop: 4 }}>
                        <Button size="small" onClick={() => {
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, ops: [...FILE_READ_OPS] } : g));
                        }}>Read Only</Button>
                        <Button size="small" onClick={() => {
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, ops: [...ALL_FILE_OPS] } : g));
                        }}>All Ops</Button>
                      </Space>
                    </div>
                  </div>

                  <div>
                    <Text type="secondary" style={{ fontSize: 12 }}>Deny Patterns (one per line)</Text>
                    <TextArea
                      autoSize={{ minRows: 2, maxRows: 4 }}
                      value={(grant.denies ?? []).join("\n")}
                      placeholder={"**/.git/**\n**/target/**"}
                      onChange={e => {
                        const denies = e.target.value.split("\n");
                        setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, denies } : g));
                      }}
                    />
                  </div>

                  <div>
                    <Text type="secondary" style={{ fontSize: 12 }}>Write Deny Patterns (one per line)</Text>
                    <TextArea
                      autoSize={{ minRows: 1, maxRows: 4 }}
                      value={(grant.write_denies ?? []).join("\n")}
                      placeholder={"**/Cargo.lock\n**/*.lock"}
                      onChange={e => {
                        const write_denies = e.target.value.split("\n");
                        setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, write_denies } : g));
                      }}
                    />
                  </div>

                  <Row gutter={12}>
                    <Col span={8}>
                      <Text type="secondary" style={{ fontSize: 12 }}>Max Read Bytes</Text>
                      <InputNumber
                        style={{ width: "100%" }}
                        value={grant.max_read_bytes}
                        placeholder="2097152"
                        min={0}
                        onChange={val => {
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, max_read_bytes: val ?? undefined } : g));
                        }}
                      />
                    </Col>
                    <Col span={8}>
                      <Text type="secondary" style={{ fontSize: 12 }}>Max Write Bytes</Text>
                      <InputNumber
                        style={{ width: "100%" }}
                        value={grant.max_write_bytes}
                        placeholder="2097152"
                        min={0}
                        onChange={val => {
                          setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, max_write_bytes: val ?? undefined } : g));
                        }}
                      />
                    </Col>
                    <Col span={8}>
                      <Space direction="vertical" size={4}>
                        <Space>
                          <Switch
                            size="small"
                            checked={grant.respect_gitignore ?? true}
                            onChange={val => {
                              setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, respect_gitignore: val } : g));
                            }}
                          />
                          <Text type="secondary" style={{ fontSize: 12 }}>Respect .gitignore</Text>
                        </Space>
                        <Space>
                          <Switch
                            size="small"
                            checked={grant.allow_overwrite ?? true}
                            onChange={val => {
                              setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, allow_overwrite: val } : g));
                            }}
                          />
                          <Text type="secondary" style={{ fontSize: 12 }}>Allow Overwrite</Text>
                        </Space>
                        <Space>
                          <Switch
                            size="small"
                            checked={grant.allow_recursive_delete ?? false}
                            onChange={val => {
                              setFileAccessEditorGrants(prev => prev.map((g, i) => i === index ? { ...g, allow_recursive_delete: val } : g));
                            }}
                          />
                          <Text type="secondary" style={{ fontSize: 12 }}>Allow Recursive Delete</Text>
                        </Space>
                      </Space>
                    </Col>
                  </Row>
                </Space>
              </Card>
            ))
          )}
        </Space>
      </Modal>
      <Modal
        open={editorOpen}
        title="Create SSH key"
        okText="Create"
        cancelText="Cancel"
        onCancel={() => setEditorOpen(false)}
        onOk={() => void handleSubmitSshForm()}
        confirmLoading={sshAction === "create"}
        destroyOnClose
      >
        <Alert
          showIcon
          type="info"
          style={{ marginBottom: 16 }}
          message="Only one active SSH key is supported, and SSH grants stay active until rotation or revoke"
          description="Creating a new key should automatically revoke the previous SSH key and its grants on the backend. SSH mode ignores time-based grant TTL, so callers keep access until you rotate or revoke this SSH key."
        />
        <Form
          form={sshForm}
          layout="vertical"
          initialValues={{ label: "", grant_mode: "permanent" satisfies GrantMode }}
        >
          <Form.Item
            label="Label"
            name="label"
            rules={[
              { required: true, message: "Please enter a label" },
              { max: 80, message: "Label must be 80 characters or fewer" },
            ]}
          >
            <Input placeholder="CI Agent" maxLength={80} />
          </Form.Item>
          <Form.Item name="grant_mode" hidden>
            <Input />
          </Form.Item>
        </Form>
      </Modal>
      <Modal
        open={secretModal !== null}
        title={secretModal?.title}
        okText="Close"
        cancelButtonProps={{ style: { display: "none" } }}
        onOk={() => setSecretModal(null)}
        onCancel={() => setSecretModal(null)}
        width={760}
      >
        <Space direction="vertical" size={12} style={{ width: "100%" }}>
          <Alert showIcon type="warning" message={secretModal?.description} />
          <Descriptions
            size="small"
            column={1}
            items={[
              {
                key: "device_code",
                label: "Device Code",
                children: <Text code>{secretModal?.payload.device_code ?? "-"}</Text>,
              },
              {
                key: "fingerprint",
                label: "Fingerprint",
                children: (
                  <Text code style={{ fontSize: 11 }}>
                    {formatSshFingerprint(secretModal?.payload.ssh_key_fingerprint)}
                  </Text>
                ),
              },
            ]}
          />
          <div>
            <Space style={{ marginBottom: 8 }}>
              <Text strong>Bifrost key file</Text>
              <Button
                size="small"
                icon={<CopyOutlined />}
                onClick={() => {
                  if (!secretModal?.payload.bifrost_key_file) return;
                  copyToClipboard(secretModal.payload.bifrost_key_file);
                  message.success("Key file copied");
                }}
              >
                Copy
              </Button>
            </Space>
            <TextArea
              readOnly
              autoSize={{ minRows: 8, maxRows: 14 }}
              value={secretModal?.payload.bifrost_key_file ?? ""}
            />
          </div>
          <div>
            <Text strong>CLI Example</Text>
            <div style={{ marginTop: 8 }}>
              <Text code>bifrost remote connect --ssh-key /path/to/bifrost.key</Text>
            </div>
          </div>
        </Space>
      </Modal>

    </div>
  );
}
