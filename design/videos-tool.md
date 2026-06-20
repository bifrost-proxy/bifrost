# Videos Tool

## Scope

Videos is an Admin Web UI tool under `AI -> TOOLS`. The first supported provider is YouTube. Users paste a YouTube URL, optionally change the download directory, and start a background download with visible progress.

The default directory is the local system Downloads directory plus `YouTube`, for example `/Users/<user>/Downloads/YouTube` on macOS. The backend accepts an absolute custom directory and also expands `~` and `~/...`.

## Backend

`bifrost-admin` owns the `/api/videos` endpoints:

- `GET /api/videos/defaults`: returns the default download directory.
- `GET /api/videos/downloads`: returns in-memory download tasks, newest first.
- `POST /api/videos/downloads`: validates the YouTube URL and directory, creates a task, and spawns `yt-dlp`.
- `GET /api/videos/downloads/{id}`: returns a single task.
- `GET /api/videos/downloads/{id}/file`: streams the completed file with byte-range support for browser playback.
- `POST /api/videos/downloads/{id}/open`: opens the completed file with the system default app.
- `POST /api/videos/downloads/{id}/reveal`: reveals the completed file in the platform file manager.
- `POST /api/videos/downloads/{id}/retry`: retries a failed task in-place.

The initial implementation stores tasks in process memory. This keeps the feature small and avoids introducing a new persistence format before cancellation, history retention, and cleanup semantics are designed.

The downloader uses `yt-dlp` as a runtime dependency. It creates the target directory, then runs `yt-dlp` with newline progress enabled so stdout can be parsed into `progress_percent`, `total`, `speed`, `eta`, and final file path. Downloads use `--continue`, so retrying a failed task reuses existing `.part` files for breakpoint resume where `yt-dlp` can resume the underlying source. The format selector prefers 8K video, then 4K video, then the best available video/audio combination:

```text
bv*[height>=4320]+ba/bv*[height>=2160]+ba/bv*+ba/b
```

If `yt-dlp` is missing, the task fails with an actionable error instead of silently doing nothing.

## Frontend

`web/src/pages/AI/VideosTool.tsx` is rendered when the `tools-videos` section is active. It follows the existing AI page navigation model and adds a `Videos` item below `TOOLS`, alongside ASR.

The page contains:

- YouTube URL input.
- Download directory input with a button to restore the default.
- Download action button.
- Task table with video URL, target path, progress bar, status, updated time, and completion actions.

The frontend polls `/api/videos/downloads` every second while any task is `queued` or `running`, and stops polling when all tasks are terminal.

Failed rows expose `Retry`, which requeues the same task and reruns `yt-dlp` with the same URL and directory so partial files can resume. Completed rows expose `Play`, `Open`, and `Reveal` actions. `Play` opens the backend file endpoint in a browser tab so the video can be played directly with native browser controls. `Open` launches the local default media app, and `Reveal` opens the containing folder with the file selected when the platform supports it.

## Validation

Core validation is split across layers:

- Rust unit tests cover YouTube URL filtering, absolute/custom directory handling, and `yt-dlp` progress parsing.
- Web lint/build checks cover TypeScript API and page rendering compile-time regressions.
- `human_tests/videos-tool.md` covers the real user flow: navigation, default directory, custom directory, progress visibility, browser playback, local file reveal/open actions, and non-YouTube rejection.

## Follow-Up Design Notes

Future iterations should consider persistent download history, task cancellation, directory picker integration for desktop builds, and explicit cleanup of partial downloads.
