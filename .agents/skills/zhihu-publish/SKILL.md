---
name: zhihu-publish
description: Publish Markdown articles to Zhihu Columns through credentials and request templates recovered locally from Bifrost traffic, save drafts, prevent duplicate posts, and verify the live title/body without browser automation. Use when the user asks to create, preview, save, publish, repost, or验收 a 知乎/知乎专栏/Zhihu article, or automate future Zhihu article releases from Markdown.
---

# Zhihu Publish

Use the bundled scripts for deterministic publishing. Do not operate Chrome or another browser through UI automation. Read authentication and request templates locally from Bifrost traffic only.

Never print or save Cookie values, XSRF tokens, `x-zse-*`, `x-zst-*`, raw request headers, or unredacted Bifrost traffic.

## Prepare the article

Copy [article.template.md](references/article.template.md) and fill `title`. The remaining fields are optional.

- Start the body with the introduction or an H2; do not repeat the title as an H1.
- Keep one source file per intended article so duplicate prevention remains stable.
- Existing frontmatter fields for another platform are allowed and ignored.
- The renderer supports headings, paragraphs, lists, links, images, inline code, fenced code blocks, blockquotes, emphasis, and horizontal rules.
- Images may use HTTP(S), a local JPEG/PNG/GIF/WebP/BMP path relative to the article, an absolute local image path, or a matching base64 data URI. Live writes transfer every non-Zhihu image to Zhihu first; SVG and files over 20 MiB are rejected.

Read [traffic-contract.md](references/traffic-contract.md) only when diagnosing a Zhihu API or capture change.

## Preview

Always run a dry run before a live write:

```bash
.agents/skills/zhihu-publish/scripts/zhihu-publish \
  --article /absolute/path/article.md \
  --dry-run
```

Confirm the title, Markdown/HTML character counts, action, duplicate state, and image counts. Dry-run never uploads an image.

Check the captured login/template state without exposing credentials:

```bash
.agents/skills/zhihu-publish/scripts/zhihu-publish --check-auth
```

## Save a draft or publish

Use `--draft-only` for a Zhihu draft. Use `--publish` only when the user explicitly requests publication.

```bash
.agents/skills/zhihu-publish/scripts/zhihu-publish \
  --article /absolute/path/article.md \
  --draft-only

.agents/skills/zhihu-publish/scripts/zhihu-publish \
  --article /absolute/path/article.md \
  --publish
```

The script performs the captured workflow:

1. Find recent Zhihu draft and publish requests with `bifrost search`.
2. Read their headers and publish payload locally with `bifrost traffic get`.
3. Create or resume a draft, then update title and HTML content.
4. Rebuild the captured publish payload with the new draft ID, title, and content.
5. Publish and store only IDs, URL, and a content hash in local state.

Treat `already_published` as successful duplicate prevention. Do not use `--force-new` unless the user explicitly wants a second post from the same source.

To change the same already-published article, require explicit user intent and pass `--update-existing`:

```bash
.agents/skills/zhihu-publish/scripts/zhihu-publish \
  --article /absolute/path/article.md \
  --publish \
  --update-existing
```

The live workflow uploads remote images through `POST /api/uploaded_images`, uploads local/data images as multipart `picture`, accepts only HTTPS URLs on Zhihu's allowlisted upload hosts (`*.zhimg.com` or the exact `pic-private.zhihu.com` host), and emits standalone images as `<figure>` nodes before saving or publishing. The private upload URL is temporary; Zhihu must rewrite the published article to stable `*.zhimg.com` content.

## Verify the live article

After publishing, compare the rendered title and body with the source without browser automation:

```bash
.agents/skills/zhihu-publish/scripts/zhihu-verify ARTICLE_ID \
  --article /absolute/path/article.md
```

The verifier reuses safe, local Bifrost request context to load Zhihu, normalizes rendered text, rejects a duplicated title at the start of the body, and requires the expected image count with every public image hosted on stable HTTPS `*.zhimg.com`. It returns `status=verified` only when text and images both match.

## Authentication recovery

The scripts first reuse the newest valid Bifrost capture. If no usable capture exists, do not copy cookies manually. Ask the user to open the Zhihu editor and perform one normal save or publish while Bifrost is running, then retry.

For a bounded capture command, use:

```bash
bifrost capture wait \
  --host zhuanlan.zhihu.com \
  --path /api/articles/drafts \
  --method POST \
  --timeout 5m \
  --format json
```

A formal publish also needs a recent `POST www.zhihu.com/api/v4/content/publish` capture because Zhihu supplies endpoint-specific signing headers and payload fields.

## Safety and validation

- Accept production API bases only on `https://zhuanlan.zhihu.com` and `https://www.zhihu.com`; allow loopback only for tests.
- Never weaken assertions or retry validation errors as authentication failures.
- Persist a new draft ID before updating it so interrupted runs resume without creating duplicates.
- Verify the live page after every real publish.
- Validate changes with:

```bash
node --test .agents/skills/zhihu-publish/scripts/*.test.mjs
python3 /Users/eden_studio/.codex/skills/.system/skill-creator/scripts/quick_validate.py \
  .agents/skills/zhihu-publish
```
