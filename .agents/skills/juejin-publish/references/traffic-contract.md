# Juejin traffic contract

This contract was derived from successful Bifrost captures. Re-check it when Juejin changes its editor or API.

## Request sequence

1. `POST https://api.juejin.cn/content_api/v1/article_draft/create?aid=2608&uuid=`
2. `POST https://api.juejin.cn/content_api/v1/article_draft/update?aid=2608&uuid=`
3. `POST https://api.juejin.cn/content_api/v1/article/publish?aid=2608&uuid=`

All requests send JSON and include browser `Cookie`, `User-Agent`, `Origin`, `Referer`, and `x-secsdk-csrf-token` headers.

## Draft fields

The create/update body contains:

- `id` on update only
- `category_id`, `tag_ids`
- `title`, `brief_content`, `mark_content`
- `link_url`, `cover_image`, `edit_type`, `html_content`
- `theme_ids`, `pics`

The successful response is `{err_no: 0, err_msg: "", data: {...}}`; the draft ID is `data.id`.

## Publish fields

The publish body contains:

- `draft_id`
- `sync_to_org`
- `column_ids`, `theme_ids`
- `origin_word_count`
- `encrypted_word_count`

The editor counts CJK/Japanese characters, Latin word runs, and individual digits after filtering images, URLs, formulas, Mermaid, and selected HTML. It masks the count with:

```text
encrypted_word_count = (origin_word_count XOR 538942) + 538942
```

The successful response returns `data.article_id`.

## Authentication

Never persist or document real values. A usable captured request has:

- a `sessionid` or `sessionid_ss` Cookie
- `x-secsdk-csrf-token`
- the complete Cookie header from the same request

Verify it with `GET /user_api/v1/user/get`. HTTP 401/403, login/session/token error numbers, or login-related error messages require a fresh browser capture.
