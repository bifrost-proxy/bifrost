# CI Flake Hunt 10x Trigger

This file intentionally lives under `scripts/` so PR path filters run the full
CI workflow for the `codex/ci-flake-hunt-10x` branch.

The branch is a CI stability probe. Product behavior should remain unchanged;
any later commits on this branch should only fix concrete CI instability found
from GitHub Actions logs.
