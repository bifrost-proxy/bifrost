# Zhihu publishing traffic contract

Read this file only when debugging a request, authentication, or response-shape change.

## Captured request sequence

1. Create draft: `POST https://zhuanlan.zhihu.com/api/articles/drafts`
   - JSON: `title`, `delta_time`, `can_reward`
   - Response: draft `id`, `url`, `state`, `title`, `content`
2. Save title/content: `PATCH https://zhuanlan.zhihu.com/api/articles/{id}/draft`
   - Title JSON: `title`, `delta_time`, `can_reward`
   - Content JSON: `content`, `table_of_contents`, `delta_time`, `can_reward`
3. Publish: `POST https://www.zhihu.com/api/v4/content/publish`
   - Top level: `action: "article"`, `data`
   - Dynamic fields: `data.draft.id`, `data.title.title`, `data.hybrid.html`, `data.hybrid.textLength`
   - `data.extra_info.pc_business_params` is a JSON string whose `title` and `content` must match.
   - Response: `code: 0`; parse JSON string `data.result`; `publish.id` is the article ID.

## Authentication material

Use captured values only in memory:

- Cookie
- `x-xsrftoken`
- `x-zse-93`, `x-zse-96`, `x-zst-81` when present
- User-Agent, Origin, Referer, `x-requested-with`

Remove transport-managed headers such as `content-length`, `host`, `accept-encoding`, and HTTP/2 pseudo headers before replaying with `fetch`.

Do not place captured values in fixtures. Tests must use synthetic tokens against loopback mocks.
