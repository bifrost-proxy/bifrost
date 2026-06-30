# ASR Directory Task Audio Directory Paths

## 功能模块说明

本用例验证 ASR Directory Task 的 `Audio Directory` 路径输入和展示规则。用户可以输入绝对路径，也可以输入 `~/xxx` 表示当前运行用户的 home 目录。为了兼容历史数据，后端读取旧任务时也会把 `~/xxx` 和普通相对路径转换为 home 下的绝对路径，避免创建或扫描 `BIFROST_DATA_DIR` / `.bifrost` 下的错误相对目录。

## 前置条件

- 在仓库根目录执行。
- 使用临时 `BIFROST_DATA_DIR` 和临时 `HOME` 启动 Bifrost。
- 启动命令必须包含 `--no-system-proxy`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`。
- 使用当前源码构建出的 `bifrost` 二进制，不复用旧安装版本。

## 测试用例列表

### TC-ASR-AUDIO-DIR-01 `~/xxx` 创建后展示为绝对路径

1. 启动临时 Bifrost 服务。
2. 调用 `POST /_bifrost/api/asr/tasks` 创建任务，body 中设置 `audio_dir` 为 `~/bifrost-asr-home-audio`。
3. 读取创建响应中的 `audio_dir`。
4. 读取 `BIFROST_DATA_DIR/asr/tasks.json` 中该任务的 `audio_dir`。

预期结果：

- 创建响应中的 `audio_dir` 等于 `<临时 HOME>/bifrost-asr-home-audio`。
- `tasks.json` 中保存的 `audio_dir` 也是同一个绝对路径。
- `<临时 HOME>/bifrost-asr-home-audio` 目录已创建。
- `BIFROST_DATA_DIR/~/bifrost-asr-home-audio` 不存在。

### TC-ASR-AUDIO-DIR-02 普通相对路径兼容转换到 home

1. 对 TC-ASR-AUDIO-DIR-01 创建的任务调用 `PATCH /_bifrost/api/asr/tasks/<task_id>`。
2. body 中设置 `audio_dir` 为 `relative-audio`。
3. 读取 PATCH 响应和 `tasks.json`。

预期结果：

- PATCH 响应中的 `audio_dir` 等于 `<临时 HOME>/relative-audio`。
- `tasks.json` 中保存的 `audio_dir` 也是同一个绝对路径。
- `<临时 HOME>/relative-audio` 目录已创建。
- `BIFROST_DATA_DIR/relative-audio` 不存在。

### TC-ASR-AUDIO-DIR-03 绝对路径保持不变

1. 创建一个临时音频目录。
2. 调用 `POST /_bifrost/api/asr/tasks` 创建任务，body 中设置 `audio_dir` 为该临时音频目录的绝对路径。
3. 读取创建响应。

预期结果：

- 创建响应中的 `audio_dir` 等于输入的绝对路径。
- 该路径不会被拼接到 home 或 `BIFROST_DATA_DIR` 下。

## 清理步骤

- 停止临时 Bifrost 服务。
- 删除临时 `BIFROST_DATA_DIR`、临时 `HOME`、临时音频目录和输出文件。

## 执行记录

| 日期 | 用例 | 命令 | 结果 |
| --- | --- | --- | --- |
| 2026-06-29 | TC-ASR-AUDIO-DIR-01/02/03 | `BIFROST_ASR_TASK_CLI_E2E_PORT=18990 bash e2e-tests/tests/test_asr_task_cli.sh` | 通过：脚本使用临时 `HOME` 和 `BIFROST_DATA_DIR` 启动当前源码构建的 Bifrost，创建 `~/bifrost-asr-home-audio` 后响应和 `tasks.json` 均为临时 home 下绝对路径；PATCH `relative-audio` 后同样落到临时 home 下；绝对路径任务保持原路径；未在数据目录下创建 `~/...` 或相对目录；原有 ASR task CLI、daily、source cleanup 回归也通过。 |
