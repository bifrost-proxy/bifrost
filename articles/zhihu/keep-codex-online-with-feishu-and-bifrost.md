---
title: "使用飞书机器人让 Codex 24 小时在线：随时随地给你的 AI 工程师派活"
comment_permission: "anyone"
disclaimer_type: "none"
table_of_contents: false
can_reward: false
source_platform: "juejin"
source_url: "https://juejin.cn/post/7672957997422346303"
source_article_id: "7672957997422346303"
source_draft_id: "7672971309705756714"
source_published_at: "2026-08-12T10:04:35.000Z"
source_category: "开发工具"
source_tags: ["OpenAI","ChatGPT","开源"]
source_brief_content: "用 Bifrost IM Gateway 把飞书变成 Codex 的远程工作入口，在手机上派活、看进度、切项目和管理任务队列。"
---

很多人已经习惯在电脑前使用 Codex：打开终端、进入仓库、描述需求，然后等待它读代码、改文件、跑测试。

但工程问题并不会只在你坐在电脑前时出现。

- 通勤路上突然想到一个需求，希望先让 Agent 分析影响面；
- 吃饭时收到 CI 告警，希望它马上定位失败日志；
- 开会时需要它在另一个仓库做只读 review；
- 下班后想确认长任务的进度，却不想远程控制整台电脑。

我想要的不是另一个聊天机器人，而是一个真正连接到开发环境的远程工作入口：手机里发一句话，家里或办公室电脑上的 Codex 就能在正确的仓库里开始工作，执行过程持续回到飞书，任务结束后还能继续追问。

Bifrost 的 IM Gateway 正是为这个场景准备的。

## 它是怎么工作的

整个链路并不复杂：

```text
飞书单聊/群聊
```text
↓
```
Bifrost IM Gateway
```text
↓
```
Codex Runner
```text
↓
```
本机仓库、终端、Git、测试与已安装 Skills
```

飞书负责“随时随地”，Codex 负责理解和执行工程任务，Bifrost 则负责把两边稳定地连接起来。

它不是把一条飞书消息简单转发给命令行。Bifrost 会维护会话、工作目录、任务队列和 Runner 绑定，把 Codex 的计划、工具调用、阶段进度和最终结果整理成飞书卡片。每个群聊都有独立上下文，不会把 A 项目的讨论串到 B 项目里。

这里所说的“24 小时在线”也不是魔法云托管：运行 Bifrost 和 Codex 的电脑必须保持开机、联网，且不能进入会中断进程的深度睡眠。你可以把它放在常开的 Mac mini、开发机或自己的服务器上。

## 第一步：让 Bifrost 常驻运行

安装好 Bifrost 和 Codex 后，先确认两者在本机可用：

```bash
bifrost --version
codex --version
```

然后让 Bifrost 以后台模式启动：

```bash
bifrost start -d
bifrost status
```

如果希望重启电脑后仍能自动恢复，可以再把 Bifrost 加入系统的登录启动项或服务管理器。关键点只有两个：Bifrost 进程持续运行，Codex 在这台机器上已经完成正常配置。

## 第二步：把飞书绑定到 Codex

Bifrost 提供交互式的飞书接入流程。最短命令只需要一个 Provider 名称、平台类型和 Runner：

```bash
bifrost im provider add feishu-main --type feishu --runner codex
```

命令会输出授权地址和二维码，并等待你在飞书中确认。授权完成后，Bifrost 会自动创建并连接 Provider，机器人会发送上线通知和可用命令帮助。

如果你已经有自己的飞书应用，也可以用环境变量引用 Secret，避免把密钥写进 shell history：

```bash
export FEISHU_APP_SECRET='你的应用密钥'

bifrost im provider add feishu-main \
  --type feishu \
  --app-id cli_xxx \
  --secret env:FEISHU_APP_SECRET \
  --owner-open-id ou_xxx \
  --runner codex
```

Provider 成功上线后，在飞书里发送：

```text
/status
```

你应该能看到当前 Runner、会话状态和工作目录。

## 第三步：把会话切到正确的仓库

远程派活最怕的一件事，是 Agent 在错误目录里工作。Bifrost 为每个会话维护独立的工作目录，可以直接在飞书里查看和切换：

```text
/pwd
/cwd /Users/you/work/my-project
```

切换后可以先发一个只读任务验证环境：

```text
请读取这个仓库的 AGENTS.md 和 README，只做分析，不修改文件。
告诉我项目的主要模块、测试入口，以及当前 git 状态。
```

确认无误后，就可以像在 Codex 桌面端或 CLI 里一样派发真实任务：

```text
修复用户列表在空数据时一直显示 loading 的问题。
先定位根因，补回归测试，完成后运行相关测试并把变更摘要发给我。
不要跳过现有 AGENTS.md 的交付要求。
```

Codex 使用的仍然是原来的本机仓库、Git 身份、工具链和项目规则。飞书只是入口，不会把你的工程复制成一套功能缩水的“聊天版开发环境”。

## 不只是最终答案：在手机上看完整执行进度

长任务最需要的不是一句“正在处理”，而是可判断的进度。

Bifrost 会把计划、工具执行和阶段结果持续更新到飞书卡片。你可以看到 Codex 正在读哪些模块、执行什么测试、是否遇到阻塞，而不必打开远程桌面盯着终端。

常用的会话命令包括：

```text
/help            查看当前通道支持的命令
/status          查看 Runner 和会话状态
/pwd             查看工作目录
/cwd <path>      切换当前会话的工作目录
/q               查看当前线程队列
/rq              查看可恢复的队列/任务
/stop            停止当前执行
/runner          查看当前 Runner
/models          查看可用模型
/efforts         查看可用 reasoning effort
/fast status     查看 Codex Fast 状态
```

具体显示哪些模型或推理强度命令，取决于当前 Codex Adapter 的能力。Bifrost 不会在不支持时伪造一个“成功”。

## 单聊与群聊：让每个项目拥有自己的 Agent 入口

个人使用时，和机器人单聊最直接；团队协作时，可以把机器人加入项目群。

Bifrost 为每个群维护独立的 Session Key、上下文游标、Runner 和工作目录。你可以让“发布群”固定绑定发布仓库，让“客户端群”绑定 App 仓库，互不干扰。

群聊里普通消息默认只进入上下文账本，不会每句话都触发模型。明确 `@机器人`、发送 `/g`、`/q` 或 slash 命令时，才会真正启动处理。这既能保留必要的讨论背景，也避免聊天刷屏导致 Agent 被频繁唤醒。

例如，在一次线上问题讨论结束后直接发送：

```text
@Bifrost 请结合上面的讨论检查仓库中对应实现，先给出根因和最小修复方案，不要立即修改。
```

确认方案后再继续：

```text
按这个方案实现，补单元测试和 E2E，完成两轮 review 后提交 PR，并跟进 CI。
```

这比把聊天记录复制到电脑、重新解释上下文，再手工启动 Agent 顺畅得多。

## 把固定工作变成定时任务

除了消息触发，IM Gateway 还支持 schedule。比如每天上午 9 点让 Codex 汇总一次流量和异常：

```bash
bifrost im target add oncall \
  --receive-id-type chat_id \
  --receive-id oc_xxx

bifrost im schedule add agent-daily \
  --target oncall \
  --cron '0 9 * * *' \
  --agent-prompt '检查最近 24 小时的代理流量，汇总错误趋势和需要关注的异常，不要修改任何配置。' \
  --agent-runner-id codex \
  --agent-model gpt-5 \
  --agent-reasoning-effort high
```

同样的机制还可以用于 CI 摘要、依赖更新检查、发布前巡检和项目日报。定时任务的输出会回到指定飞书目标，不需要你每天手工发同一句 prompt。

## 让“随时可派活”保持安全

远程控制工程 Agent 很方便，也意味着需要认真设置边界。

我的建议是：

1. **默认使用受限权限。** 不要为了省事把所有任务都配置成 danger-full-access。
2. **为不同群绑定不同仓库。** 用 `/cwd` 明确工作范围，不要让所有会话共享一个宽泛目录。
3. **敏感值使用环境变量引用。** 飞书 Secret、Token 不写进命令历史、文章或仓库。
4. **保留人工确认。** 发布、删除、生产变更等高风险动作，仍要求 Agent 在执行前确认。
5. **先分析，再修改。** 对陌生问题先下达只读诊断任务，确认方案后再授权实现。
6. **让项目规则继续生效。** 在仓库中维护清晰的 AGENTS.md、测试和提交要求，远程入口也会遵守它们。

“24 小时在线”不应该等于“24 小时无限权限”。稳定的远程 Agent，靠的是随时可达、上下文连续、状态透明，以及清晰的授权边界。

## 最后

把飞书接到 Codex 之后，我最明显的感受是：很多以前必须“回到电脑前再说”的事情，现在可以立刻进入队列。

想到需求时先让它分析；收到告警时先让它采证；离开电脑时继续看测试和 CI；需要暂停时发 `/stop`。电脑仍然是完整的工程执行环境，而手机成了随身的任务控制台。

Bifrost 是开源项目，仓库地址：

[https://github.com/bifrost-proxy/bifrost](https://github.com/bifrost-proxy/bifrost)

如果你也在使用 Codex、Claude Code 或 Trae，可以尝试把常驻开发机接入 IM Gateway。下一次灵感或故障到来时，不必等到坐回电脑前，直接在飞书里把活派出去。
