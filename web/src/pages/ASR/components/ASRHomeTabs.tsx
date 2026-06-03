import type { Dispatch, RefObject, SetStateAction } from "react";
import type { FormInstance } from "antd";
import { Tabs, theme } from "antd";
import type {
  AsrConnectionParams,
  AsrDirectoryTask,
  AsrExternalImportRunProgress,
  AsrOfflineJob,
  AsrPauseMode,
  AsrStatus,
  SpeechPipelinesStatus,
  VoiceRealtimeTranscriptSegment,
} from "../../../api/asr";
import SpeechTab from "../../Settings/tabs/SpeechTab";
import type { WorkState } from "../asrUtils";
import DiarizationSetupCard from "./DiarizationSetupCard";
import DirectoryTasksPanel from "./DirectoryTasksPanel";
import SpeechWorkbench from "./SpeechWorkbench";
import VoiceWakeActionsCard from "./VoiceWakeActionsCard";
import { isAsrHomeTabKey, type AsrHomeTabKey } from "./asrHomeRoute";

interface ASRHomeTabsProps {
  activeTab: AsrHomeTabKey;
  onChangeTab: (tab: AsrHomeTabKey) => void;
  token: ReturnType<typeof theme.useToken>["token"];
  taskForm: FormInstance;
  taskScheduleKind: string;
  tasks: AsrDirectoryTask[];
  tasksLoading: boolean;
  externalImportProgressByTask: Record<string, AsrExternalImportRunProgress>;
  onCreateTask: () => boolean | Promise<boolean>;
  onUpdateTask: (id: string) => boolean | Promise<boolean>;
  onRunExternalImport: (id: string) => void | Promise<void>;
  onOpenTask: (id: string) => void;
  onRunTask: (id: string) => void;
  onPauseTask: (id: string, force?: boolean, mode?: AsrPauseMode) => void;
  onResumeTask: (id: string) => void;
  onRemoveTask: (id: string, confirmName: string) => void;
  ready: boolean | undefined;
  status: AsrStatus | null;
  params: AsrConnectionParams;
  onParamsChange: Dispatch<SetStateAction<AsrConnectionParams>>;
  serviceBusy: boolean;
  workState: WorkState;
  progress: number;
  selectedName: string;
  transcript: string;
  realtimeSegments: VoiceRealtimeTranscriptSegment[];
  partialRealtimeSegment: VoiceRealtimeTranscriptSegment | null;
  offlineJob: AsrOfflineJob | null;
  offlineArtifacts: Record<string, string>;
  speechStatus: SpeechPipelinesStatus | null;
  events: string[];
  errorText: string;
  micLevels: number[];
  micPeak: number;
  fileInputRef: RefObject<HTMLInputElement | null>;
  busy: boolean;
  showFileProgress: boolean;
  onFile: (file: File) => void;
  onStartRecording: () => void;
  onStopRecording: () => void;
  onStartService: () => void;
  onStopService: () => void;
  onCancel: () => void;
}

export default function ASRHomeTabs({
  activeTab,
  onChangeTab,
  token,
  taskForm,
  taskScheduleKind,
  tasks,
  tasksLoading,
  externalImportProgressByTask,
  onCreateTask,
  onUpdateTask,
  onRunExternalImport,
  onOpenTask,
  onRunTask,
  onPauseTask,
  onResumeTask,
  onRemoveTask,
  ready,
  status,
  params,
  onParamsChange,
  serviceBusy,
  workState,
  progress,
  selectedName,
  transcript,
  realtimeSegments,
  partialRealtimeSegment,
  offlineJob,
  offlineArtifacts,
  speechStatus,
  events,
  errorText,
  micLevels,
  micPeak,
  fileInputRef,
  busy,
  showFileProgress,
  onFile,
  onStartRecording,
  onStopRecording,
  onStartService,
  onStopService,
  onCancel,
}: ASRHomeTabsProps) {
  return (
    <div style={{ height: "100%", overflow: "auto" }} data-testid="asr-home-tabs">
      <Tabs
        activeKey={activeTab}
        onChange={(key) => {
          if (isAsrHomeTabKey(key)) {
            onChangeTab(key);
          }
        }}
        destroyOnHidden
        items={[
          {
            key: "scheduled",
            label: "Scheduled Tasks",
            children: (
              <div data-testid="asr-home-tab-scheduled">
                <DirectoryTasksPanel
                  taskForm={taskForm}
                  taskScheduleKind={taskScheduleKind}
                  tasks={tasks}
                  tasksLoading={tasksLoading}
                  externalImportProgressByTask={externalImportProgressByTask}
                  onCreateTask={onCreateTask}
                  onUpdateTask={onUpdateTask}
                  onRunExternalImport={onRunExternalImport}
                  onOpenTask={onOpenTask}
                  onRunTask={onRunTask}
                  onPauseTask={onPauseTask}
                  onResumeTask={onResumeTask}
                  onRemoveTask={onRemoveTask}
                />
              </div>
            ),
          },
          {
            key: "management",
            label: "ASR Management",
            children: (
              <div data-testid="asr-home-tab-management">
                <SpeechTab />
                <SpeechWorkbench
                  token={token}
                  ready={ready}
                  status={status}
                  params={params}
                  onParamsChange={onParamsChange}
                  serviceBusy={serviceBusy}
                  workState={workState}
                  progress={progress}
                  selectedName={selectedName}
                  transcript={transcript}
                  realtimeSegments={realtimeSegments}
                  partialRealtimeSegment={partialRealtimeSegment}
                  offlineJob={offlineJob}
                  offlineArtifacts={offlineArtifacts}
                  speechStatus={speechStatus}
                  events={events}
                  errorText={errorText}
                  micLevels={micLevels}
                  micPeak={micPeak}
                  fileInputRef={fileInputRef}
                  busy={busy}
                  showFileProgress={showFileProgress}
                  onFile={onFile}
                  onStartRecording={onStartRecording}
                  onStopRecording={onStopRecording}
                  onStartService={onStartService}
                  onStopService={onStopService}
                  onCancel={onCancel}
                />
              </div>
            ),
          },
          {
            key: "voice",
            label: "Voiceprint & Wake",
            children: (
              <div data-testid="asr-home-tab-voice">
                <DiarizationSetupCard />
                <VoiceWakeActionsCard />
              </div>
            ),
          },
        ]}
      />
    </div>
  );
}
