# ASR Daily Agent Failure Isolation

## Context

Daily Agent runs can process multiple changed daily Markdown files in one run. The ChatGPT Web adapter processes those changed files one date at a time, so a single stuck or timed-out date should not stop later dates from generating reports.

Previously, any per-date ChatGPT Web error propagated out of the loop and marked the entire run as failed. Reports saved before the failing date remained on disk, but run status and `reports_generated` did not reflect the partial progress.

## Goals

- Keep global setup failures fail-fast: workspace setup, change planning, runner lookup, and non-ChatGPT batch runner failures still fail the whole run.
- Isolate ChatGPT Web per-date failures: one date can fail while the run continues with remaining changed dates.
- Preserve successful report accounting: already-generated reports remain in `reports_generated` and are written to processed state.
- Surface partial failure clearly: runs with both generated reports and failed dates use `partial_success`, with an error summary listing failed dates and report targets.
- Shorten the default Daily Agent timeout from two hours to one hour. ChatGPT Web inner timeouts keep the existing 30 second headroom.

## Behavior

For ChatGPT Web Daily Agent runs:

1. Build a single-entry plan per changed date.
2. Send the date prompt, validate the response, and use the existing same-conversation wait, retry, and continuation flow.
3. On success, write that date report and continue.
4. On failure, record `date`, `report_target`, and `error`, log the failure, and continue with the next date.
5. After all dates finish, validate required reports while excluding known failed targets.
6. Persist processed state only for generated reports.

Final status:

- `success`: no failed entries.
- `partial_success`: at least one report was generated and at least one entry failed.
- `failed`: all entries failed, or a global failure prevented useful execution.

IM delivery keeps the run status distinct from report availability. `OnSuccess`
still requires a full `success`, while `OnSuccessWithReport` sends generated
reports for both `success` and `partial_success`.

## Validation

- Unit tests cover default timeout, ChatGPT Web timeout headroom, failed-entry summaries, and report gate exclusion.
- E2E shell guard checks that the ChatGPT Web Daily Agent path contains the per-entry failure continuation log and `partial_success` status.
- Human test `TC-ADA-20` covers the user-visible recovery behavior after one Daily Agent date fails.
