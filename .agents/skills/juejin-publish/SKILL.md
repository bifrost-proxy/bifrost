---
name: juejin-publish
description: Publish Markdown articles and Bifrost capability series to Juejin, or verify with Microsoft Edge that a published Juejin page's title and body match the local Markdown. Use when the user asks to create, save, publish, or验收 a 掘金/Juejin article, automate future Bifrost article releases, reuse authentication from Bifrost traffic, recover an expired/missing Juejin Cookie, or compare a live post with its source.
---

# Juejin Publish

Use the bundled script for deterministic publishing. Never paste Cookie, CSRF tokens, raw request headers, or unredacted Bifrost traffic into chat, logs, source files, or article content.

## Prepare the article

Copy [article.template.md](references/article.template.md) and fill the frontmatter:

- Require `title`, nonzero `category_id`, and at least one numeric `tag_ids` entry.
- Keep `brief_content` at 100 characters or fewer; omit it to derive a summary.
- Use JSON arrays for `tag_ids`, `column_ids`, `theme_ids`, and `pics`.
- Start with the introduction or an H2 section; do not repeat the frontmatter title as the first Markdown H1 because Juejin renders its own article heading.
- Keep one source file per intended Juejin article. The script recognizes both the source path and content hash after publication to prevent duplicates if a file is moved or copied.

Read [traffic-contract.md](references/traffic-contract.md) only when debugging an API or authentication change.

## Preview

Always preview before a live write:

```bash
.agents/skills/juejin-publish/scripts/juejin-publish \
  --article /absolute/path/to/article.md \
  --dry-run
```

Check the title, category, tags, action, and word counts in the JSON output. The dry run does not access Bifrost or Juejin.

Use the read-only authentication check when diagnosing the captured session:

```bash
.agents/skills/juejin-publish/scripts/juejin-publish --check-auth
```

It reports only the source request ID and Cookie names, never credential values.

## Save a draft or publish

Use `--draft-only` when the user asks for a draft. Use `--publish` only when the user explicitly asks to publish:

```bash
# Save or update a draft
.agents/skills/juejin-publish/scripts/juejin-publish \
  --article /absolute/path/to/article.md \
  --draft-only

# Publish
.agents/skills/juejin-publish/scripts/juejin-publish \
  --article /absolute/path/to/article.md \
  --publish
```

The script performs the observed sequence:

1. Find the newest Juejin request with `bifrost search`.
2. Read Cookie and `x-secsdk-csrf-token` locally with `bifrost traffic get`.
3. Verify authentication against the Juejin user endpoint.
4. Create or reuse a draft, update all metadata/content, then publish.
5. Save only draft/article IDs and a content hash to the local state file.

Report the returned article URL. Treat `already_published` as a successful duplicate-prevention result.

## Verify the published page with Edge

After publishing or updating an article, pass only its numeric article ID to confirm that Edge can load the rendered title and body:

```bash
.agents/skills/juejin-publish/scripts/juejin-verify 1234567890
```

The verifier launches installed Microsoft Edge, navigates exactly once, waits for the rendered article title and body, keeps the loaded window visible for 5 seconds, then closes Edge and exits 0 with `status=loaded`, the HTTP status, title, body character count, `navigation_count=1`, and `close_delay_ms=5000`.

To additionally compare the live title/body with the source Markdown, pass `--article`:

```bash
.agents/skills/juejin-publish/scripts/juejin-verify 1234567890 \
  --article /absolute/path/to/article.md
```

Strict comparison normalizes Markdown/rendered list syntax and requires an exact text match. It also rejects an unexpected duplicate title at the start of the body. A match returns `status=verified`; any mismatch exits 1 with the first differing text window.

This verification path never reads Bifrost traffic, captures Cookie values, or replays requests. It loads the page once by default and reloads only when `--refresh [N]` is explicitly supplied. Use `--headless` only for automated tests; the default opens a visible Edge window.
Use `--close-delay-ms 0` only in automated tests that do not need the visible stability window.

For a bounded reload test followed by one final content check, pass a refresh count. Omitting the count defaults to one refresh:

```bash
.agents/skills/juejin-publish/scripts/juejin-verify 1234567890 --refresh
.agents/skills/juejin-publish/scripts/juejin-verify 1234567890 --refresh 3
```

The verifier waits 3 seconds before each requested reload and prints progress for every reload. It does not inspect article content between reloads. After all reloads finish, it checks the final page once for a non-empty title and body; when `--article` is supplied, that final snapshot is also compared with the local Markdown. `refresh.complete=true` and one entry per reload in `refresh.attempts` confirm that every requested reload completed. A navigation or HTTP failure stops the run because the page can no longer be checked. Use `--refresh-delay-ms` to tune the interval. The refresh count is intentionally bounded to 1–1000 to prevent accidental high-volume traffic.

## Automatic Cookie recovery

If no usable capture exists or Juejin rejects it, allow the default recovery flow. The script runs `bifrost capture wait --open`, opens the Juejin editor in the system browser, waits for the user to log in, extracts the new request locally, validates it, and resumes the interrupted operation.

Use `--no-browser` only for CI or when the user forbids browser interaction. Do not create a separate Cookie file and do not manually copy Cookie values from traffic.

## Safety and retry rules

- Do not use `--force-new` unless the user explicitly wants a second article from the same source file.
- Do not retry non-auth API errors as authentication failures.
- On an auth failure during create/update/publish, refresh once and retry only the failed operation.
- The script rejects conflicting publication modes and persists a newly created draft ID before update, so a transient update failure can resume without creating another draft.
- Keep the default state file under the user's local state directory; it contains no credentials.
- Use `JUEJIN_PUBLISH_STATE_FILE` or `--state-file` only for isolation/testing.
- `JUEJIN_API_BASE` and `--api-base` accept only `https://api.juejin.cn` or a loopback mock endpoint, preventing credentials from being sent to another host.

## Validate the skill

```bash
node --test .agents/skills/juejin-publish/scripts/*.test.mjs
python3 /Users/eden/.codex/skills/.system/skill-creator/scripts/quick_validate.py \
  .agents/skills/juejin-publish
```
