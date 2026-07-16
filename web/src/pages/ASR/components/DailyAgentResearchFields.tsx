import { useEffect, useState } from "react";
import {
  Button,
  Descriptions,
  Input,
  InputNumber,
  message,
  Select,
  Space,
  Switch,
  Typography,
} from "antd";
import { PlusOutlined } from "@ant-design/icons";
import type { AsrDailyAgentItem } from "../../../api/asr";

const { Text } = Typography;
const DAILY_AGENT_TOKEN_RE = /^[A-Za-z0-9_-]+$/;

function normalizeAgentToken(value: string): string {
  return value
    .trim()
    .replace(/[^A-Za-z0-9_-]/g, "_")
    .replace(/^[_-]+|[_-]+$/g, "");
}

interface RunnerOption {
  label: string;
  value: string;
}

interface DailyAgentResearchFieldsProps {
  agent: AsrDailyAgentItem;
  runnerOptions: RunnerOption[];
  onChange: (patch: Partial<AsrDailyAgentItem>) => void;
}

export function DailyAgentResearchFields({
  agent,
  runnerOptions,
  onChange,
}: DailyAgentResearchFieldsProps) {
  const [contextProfileDraft, setContextProfileDraft] = useState<{
    id: string;
    runner: string;
    work_dir: string;
    instructions: string;
  } | null>(null);

  useEffect(() => {
    setContextProfileDraft(null);
  }, [agent.id]);

  return (
    <Descriptions
      column={2}
      size="small"
      bordered
      style={{ marginTop: 12 }}
    >
      <Descriptions.Item label="ChatGPT Project URL" span={2}>
        <Input
          key={`${agent.id}-daily-project-url-${
            agent.chatgpt_project_url || ""
          }`}
          data-testid="asr-daily-agent-project-url"
          size="small"
          defaultValue={agent.chatgpt_project_url || ""}
          placeholder="https://chatgpt.com/g/g-p-.../project"
          onBlur={(event) => {
            const projectUrl = event.target.value.trim();
            if (projectUrl === (agent.chatgpt_project_url || "")) {
              return;
            }
            onChange({ chatgpt_project_url: projectUrl || undefined });
          }}
          onPressEnter={(event) => event.currentTarget.blur()}
        />
        <Text type="secondary" style={{ display: "block", fontSize: 11 }}>
          Only new ChatGPT Web conversations use this Project. Existing reports
          are not migrated.
        </Text>
      </Descriptions.Item>
            <Descriptions.Item label="Independent Research">
              <Switch
                data-testid="asr-daily-agent-research-fanout-switch"
                checked={Boolean(agent.research_fanout)}
                onChange={(enabled) =>
                  onChange({
                    research_fanout: enabled
                      ? {
                          max_questions: 8,
                          max_concurrency: 3,
                          chatgpt_interface_mode: "chat",
                          chatgpt_model: "pro",
                          chatgpt_project_url: undefined,
                          allowed_runners: agent.runner
                            ? [agent.runner]
                            : [],
                          context_profiles: {},
                        }
                      : undefined,
                  })
                }
              />
            </Descriptions.Item>
            {agent.research_fanout ? (
              <>
                <Descriptions.Item label="Max Questions">
                  <InputNumber
                    data-testid="asr-daily-agent-research-max-questions"
                    size="small"
                    min={1}
                    max={50}
                    value={agent.research_fanout.max_questions}
                    onChange={(maxQuestions) =>
                      onChange({
                        research_fanout: {
                          ...agent.research_fanout!,
                          max_questions: Number(maxQuestions || 1),
                        },
                      })
                    }
                  />
                </Descriptions.Item>
                <Descriptions.Item label="Max Concurrent Research">
                  <InputNumber
                    data-testid="asr-daily-agent-research-max-concurrency"
                    size="small"
                    min={1}
                    max={8}
                    value={agent.research_fanout.max_concurrency}
                    onChange={(maxConcurrency) =>
                      onChange({
                        research_fanout: {
                          ...agent.research_fanout!,
                          max_concurrency: Number(maxConcurrency || 1),
                        },
                      })
                    }
                  />
                </Descriptions.Item>
                <Descriptions.Item label="ChatGPT Project URL" span={2}>
                  <Input
                    key={`${agent.id}-project-url-${
                      agent.research_fanout.chatgpt_project_url || ""
                    }`}
                    data-testid="asr-daily-agent-research-project-url"
                    size="small"
                    defaultValue={
                      agent.research_fanout.chatgpt_project_url || ""
                    }
                    placeholder="https://chatgpt.com/g/g-p-.../project"
                    onBlur={(event) => {
                      const chatgptProjectUrl = event.target.value.trim();
                      if (
                        chatgptProjectUrl ===
                        (agent.research_fanout!.chatgpt_project_url ||
                          "")
                      ) {
                        return;
                      }
                      onChange({
                        research_fanout: {
                          ...agent.research_fanout!,
                          chatgpt_project_url: chatgptProjectUrl || undefined,
                        },
                      });
                    }}
                    onPressEnter={(event) => event.currentTarget.blur()}
                  />
                </Descriptions.Item>
                <Descriptions.Item label="Allowed Research Runners" span={2}>
                  <Select
                    data-testid="asr-daily-agent-research-runners"
                    mode="multiple"
                    allowClear
                    value={agent.research_fanout.allowed_runners}
                    options={runnerOptions}
                    onChange={(allowedRunners: string[]) =>
                      onChange({
                        research_fanout: {
                          ...agent.research_fanout!,
                          allowed_runners: allowedRunners,
                        },
                      })
                    }
                    style={{ width: "100%" }}
                  />
                </Descriptions.Item>
                <Descriptions.Item label="Runtime Data Fallbacks" span={2}>
                  <Space
                    direction="vertical"
                    size={8}
                    style={{ width: "100%" }}
                  >
                    {Object.entries(
                      agent.research_fanout.context_profiles || {},
                    ).map(([profileId, profile]) => (
                      <Space key={profileId} wrap style={{ width: "100%" }}>
                        <Text code>{profileId}</Text>
                        <Select
                          aria-label={`${profileId} fallback runner`}
                          size="small"
                          value={profile.runner || undefined}
                          placeholder="Runner"
                          options={runnerOptions}
                          onChange={(runner) =>
                            onChange({
                              research_fanout: {
                                ...agent.research_fanout!,
                                context_profiles: {
                                  ...agent.research_fanout!
                                    .context_profiles,
                                  [profileId]: { ...profile, runner },
                                },
                              },
                            })
                          }
                          style={{ width: 180 }}
                        />
                        <Input
                          key={`${profileId}-work-dir-${profile.work_dir}`}
                          aria-label={`${profileId} fallback working folder`}
                          size="small"
                          defaultValue={profile.work_dir}
                          placeholder="Local working folder"
                          onBlur={(event) => {
                            const workDir = event.target.value.trim();
                            if (workDir === profile.work_dir) {
                              return;
                            }
                            onChange({
                              research_fanout: {
                                ...agent.research_fanout!,
                                context_profiles: {
                                  ...agent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    ...profile,
                                    work_dir: workDir,
                                  },
                                },
                              },
                            });
                          }}
                          onPressEnter={(event) => event.currentTarget.blur()}
                          style={{ width: 260 }}
                        />
                        <Input
                          key={`${profileId}-instructions-${
                            profile.instructions || ""
                          }`}
                          aria-label={`${profileId} fallback instructions`}
                          size="small"
                          defaultValue={profile.instructions || ""}
                          placeholder="What facts should be collected?"
                          onBlur={(event) => {
                            const instructions = event.target.value.trim();
                            if (instructions === (profile.instructions || "")) {
                              return;
                            }
                            onChange({
                              research_fanout: {
                                ...agent.research_fanout!,
                                context_profiles: {
                                  ...agent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    ...profile,
                                    instructions: instructions || undefined,
                                  },
                                },
                              },
                            });
                          }}
                          onPressEnter={(event) => event.currentTarget.blur()}
                          style={{ width: 260 }}
                        />
                        <Button
                          size="small"
                          danger
                          onClick={() => {
                            const nextProfiles = {
                              ...agent.research_fanout!
                                .context_profiles,
                            };
                            delete nextProfiles[profileId];
                            onChange({
                              research_fanout: {
                                ...agent.research_fanout!,
                                context_profiles: nextProfiles,
                              },
                            });
                          }}
                        >
                          Remove
                        </Button>
                      </Space>
                    ))}
                    {contextProfileDraft ? (
                      <Space wrap style={{ width: "100%" }}>
                        <Input
                          aria-label="New fallback name"
                          size="small"
                          value={contextProfileDraft.id}
                          placeholder="Fallback name"
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              id: normalizeAgentToken(event.target.value),
                            })
                          }
                          style={{ width: 160 }}
                        />
                        <Select
                          aria-label="New fallback runner"
                          size="small"
                          value={contextProfileDraft.runner || undefined}
                          placeholder="Runner"
                          options={runnerOptions}
                          onChange={(runner) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              runner,
                            })
                          }
                          style={{ width: 180 }}
                        />
                        <Input
                          aria-label="New fallback working folder"
                          size="small"
                          value={contextProfileDraft.work_dir}
                          placeholder="Local working folder"
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              work_dir: event.target.value,
                            })
                          }
                          style={{ width: 260 }}
                        />
                        <Input
                          aria-label="New fallback instructions"
                          size="small"
                          value={contextProfileDraft.instructions}
                          placeholder="What facts should be collected?"
                          onChange={(event) =>
                            setContextProfileDraft({
                              ...contextProfileDraft,
                              instructions: event.target.value,
                            })
                          }
                          style={{ width: 260 }}
                        />
                        <Button
                          size="small"
                          type="primary"
                          onClick={() => {
                            const profileId = contextProfileDraft.id.trim();
                            const runner = contextProfileDraft.runner.trim();
                            const workDir = contextProfileDraft.work_dir.trim();
                            if (
                              !DAILY_AGENT_TOKEN_RE.test(profileId) ||
                              !runner ||
                              !workDir
                            ) {
                              message.warning(
                                "Fallback name, runner, and local working folder are required",
                              );
                              return;
                            }
                            onChange({
                              research_fanout: {
                                ...agent.research_fanout!,
                                context_profiles: {
                                  ...agent.research_fanout!
                                    .context_profiles,
                                  [profileId]: {
                                    runner,
                                    work_dir: workDir,
                                    instructions:
                                      contextProfileDraft.instructions.trim() ||
                                      undefined,
                                  },
                                },
                              },
                            });
                            setContextProfileDraft(null);
                          }}
                        >
                          Save fallback
                        </Button>
                        <Button
                          size="small"
                          onClick={() => setContextProfileDraft(null)}
                        >
                          Cancel
                        </Button>
                      </Space>
                    ) : null}
                    <Button
                      data-testid="asr-daily-agent-add-context-profile"
                      size="small"
                      icon={<PlusOutlined />}
                      onClick={() => {
                        const profiles =
                          agent.research_fanout!.context_profiles || {};
                        let index = Object.keys(profiles).length + 1;
                        let profileId = `context_${index}`;
                        while (profiles[profileId]) {
                          index += 1;
                          profileId = `context_${index}`;
                        }
                        setContextProfileDraft({
                          id: profileId,
                          runner:
                            agent.research_fanout!.allowed_runners[0] ||
                            agent.runner,
                          work_dir: "",
                          instructions: "",
                        });
                      }}
                      disabled={Boolean(contextProfileDraft)}
                    >
                      Add runtime fallback
                    </Button>
                  </Space>
                </Descriptions.Item>
              </>
            ) : null}
    </Descriptions>
  );
}
