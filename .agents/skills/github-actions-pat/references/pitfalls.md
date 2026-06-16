# GitHub Actions PAT Pitfalls

这份清单是本 skill 在真实跑 CI 分析时踩过的坑与对应处置方式，按优先级从高到低列：

## 1. Azure Blob signed URL 不能带 `Authorization`

**现象**：访问 `GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs` 后 GitHub 返回 302
到 `*.blob.core.windows.net` 的签名 URL；如果 HTTP client 自动跟随 redirect 并继续带
`Authorization: Bearer ...`，Azure 返回：

```
401 InvalidAuthenticationInfo — Server failed to authenticate the request.
Make sure the value of Authorization header is formed correctly including the signature.
```

**原因**：Azure Blob 用 URL 中的签名（SAS）做鉴权，它不认识 GitHub 的 Bearer token，
但只要 `Authorization` 头存在就优先按头鉴权 → 签名失败。

**处置**（本 skill `scripts/gh_ci.py::_fetch_job_log` 已这样做）：

```python
class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def http_error_302(self, req, fp, code, msg, headers):
        return None
    http_error_301 = http_error_303 = http_error_307 = http_error_302

opener = urllib.request.build_opener(_NoRedirect())
req = urllib.request.Request(api_url, headers=_base_headers(token))
try:
    resp = opener.open(req); return resp.read().decode("utf-8", errors="replace")
except urllib.error.HTTPError as e:
    if e.code in (301, 302, 303, 307, 308):
        signed_url = e.headers.get("Location")

# 二次请求：只带 User-Agent，不带 Authorization
req2 = urllib.request.Request(signed_url, headers={"User-Agent": "..."})
with urllib.request.urlopen(req2) as resp2:
    return resp2.read().decode("utf-8", errors="replace")
```

## 2. `Accept` 头不能写 `text/plain`

**现象**：访问 `/actions/jobs/{id}/logs` 时如果显式设 `Accept: text/plain`，服务端返回：

```
415 Unsupported 'Accept' header: [...]. Must accept 'application/vnd.github+json'.
```

**处置**：使用默认 `application/vnd.github+json`。服务端会 302 到签名 URL，body 是纯文本，
直接 `bytes.decode("utf-8", errors="replace")` 即可。

## 3. System proxy 会导致 SSL 证书 MITM 失败

**现象**：用户本机挂了代理（包括 bifrost 自己）时，Python `urllib` 可能报：

```
ssl.SSLCertVerificationError: [SSL: CERTIFICATE_VERIFY_FAILED]
certificate verify failed: Missing Authority Key Identifier
```

**原因**：代理做 TLS MITM 重签的证书缺字段，Python 严格校验会拒绝。

**处置**：跑脚本前清掉代理环境变量 + 明确绕开目标域名：

```bash
NO_PROXY=api.github.com,github.com,*.blob.core.windows.net \
HTTPS_PROXY= HTTP_PROXY= ALL_PROXY= https_proxy= http_proxy= all_proxy= \
python3 scripts/gh_ci.py run <id>
```

## 4. PII / DLP mask 会吞掉 token 原文

**现象**：如果用户直接在聊天里粘 `ghp_xxx...` / `github_pat_xxx...`，平台 DLP
会把它替换成占位符（`[ph_PASSWORD_N_ph]`）。Agent 拿不到明文。

**正确做法**：

1. 让用户在本机自己 `export GITHUB_TOKEN=...` 到 shell rc（`~/.zshrc` / `~/.bashrc`）
2. Agent 通过 `zsh -ic` / `bash -lc` 加载 rc 后再执行脚本
3. 绝不把 token 贴进聊天 / 日志 / URL

## 5. `zsh -lc` 不会加载 `~/.zshrc`

**现象**：在 macOS 上用 `zsh -lc '...'` 跑脚本时，`$GITHUB_TOKEN` 为空，token_or_die 退出 2。

**原因**：`-l` 只加载 login 相关的 `.zprofile`、`.zlogin`；交互式配置在 `.zshrc` 里。
`-l` 而非 `-i` 不会把 shell 标记为 interactive，`.zshrc` 默认 skip。

**处置**：用 `zsh -ic '...'`（interactive 模式）。

## 6. bifrost remote exec 的 quoting 噩梦

**现象**：在 bifrost remote 目标机上跑 `python3 -c "import urllib; ..."`
时，`\"` 的反斜杠经过 ssh + bifrost 中继会叠到 3 层转义，Python 语法直接崩。

**处置**：永远把探针脚本写成文件后再执行：

```bash
bifrost remote file write --path /tmp/probe.py --content-b64 "$B64"
bifrost remote exec -- python3 /tmp/probe.py
```

## 7. Run log (run-level) vs job log (job-level)

- `/actions/runs/{id}/logs` → zip 归档，需解压
- `/actions/jobs/{id}/logs` → 纯文本（经 302 → 签名 URL）

**本 skill 默认走 job 级别**：定位失败原因更快，且不需要 `zipfile` 处理。只有在想对
workflow 做整体取证时再用 run-level。

## 8. Rate limit

fine-grained PAT 默认 5000 req/hour 每仓库，粒度够用。命中限流时响应头 `x-ratelimit-reset`
是 epoch 秒，等到那个时间点再重试（本 skill `common._die_http` 已打印提示）。

## 9. `filter=latest` 必加

`/actions/runs/{id}/jobs` 默认返回所有 attempt 的所有 job，会让同一 job_name 出现多次
（rerun、matrix 重启等）。加 `filter=latest` 只返回最后一次 attempt。

## 10. token scope 最小化

- 只做分析：`repo` + `actions:read`（只读）
- 要 `--post` review：追加 `pull_requests:write`
- 推荐用 **fine-grained PAT** 并限定到目标仓库，爆炸半径最小

## 11. 绝不回显 token

- 不 `echo $GITHUB_TOKEN`
- 不把 token 写进 commit / log / URL / header dump
- 本 skill 所有 HTTP 错误分支都只打印状态码 + rate-limit 头 + body 前 200 字符，不回显请求头
