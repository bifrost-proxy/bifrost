# Sync Settings Remote URL Draft Preservation

## Module

The Settings Sync tab shows the current Sync status and lets users edit the remote Sync service URL. The page polls `/_bifrost/api/sync/status` while the Sync tab or Remote Invoke tab is open so connectivity, session, and last-sync state stay fresh.

## Problem

The status polling path copied `status.remote_base_url` into the input draft on every refresh. While the user was typing a new URL, the next 2-second poll restored the old server value and made the field effectively uneditable.

## Implementation

- Keep `syncRemoteBaseUrlDraft` as the controlled input value.
- Track unsaved local edits with `syncRemoteBaseUrlDirtyRef`.
- Use one `applySyncStatus(status, options)` helper for all Sync status updates.
- Passive status refreshes update status cards, buttons, and the global Sync store, but only update the URL draft when the draft is not dirty.
- The successful `Save` action calls `applySyncStatus(..., { syncRemoteBaseUrlDraft: true })` so the URL returned by the server is written back and the dirty marker is cleared.
- Other successful Sync actions (Enable Sync, Auto Sync, Sign In, Sign Out, Sync Now) refresh status without forcing the Remote URL draft. If the draft is clean they still reflect the latest server URL; if the user is editing, they keep the local draft intact.

## Dependencies

- `web/src/pages/Settings/index.tsx`
- `web/src/pages/Settings/tabs/SyncTab.tsx`
- `web/src/api/sync.ts`

No backend API contract changes are required.

## Test Plan

- Unit/UI test:
  - `web/tests/ui/admin-settings.spec.ts`: mock `/sync/status` polling, type a draft Remote URL, switch the mocked status from unauthorized to ready, and assert the input still contains the user draft.
  - In the same test, click Save, assert `PUT /sync/config` receives the user draft, and assert the server response URL is written back into the input.
- E2E test:
  - Run the focused Playwright test for Settings Sync draft preservation.
- Real scenario test:
  - Update `human_tests/webui-settings.md` with `TC-WST-39`.
  - Execute the case against the Playwright/UI route and verify the input no longer rolls back during polling.

## Review/Fix/Test Loop

- Round 1:
  - Recheck the user symptom and current diff.
  - Review dirty marker lifetime, passive polling behavior, and explicit action paths.
  - Run the focused Playwright test and fix any failed assertion.
- Round 2:
  - Recheck the latest diff after Round 1.
  - Verify human_tests index and design document are consistent with the implementation.
  - Rerun the focused Playwright test and relevant web checks.

## Validation Requirements

- Run the focused Playwright regression first.
- Run relevant web type/build checks.
- Run project validation after E2E per repo rules.
- Run coverage gate at closeout, or document why only the unit coverage fallback could run.

## Documentation

- `design/sync-settings-draft-preservation.md`
- `human_tests/webui-settings.md`
- `human_tests/readme.md`
