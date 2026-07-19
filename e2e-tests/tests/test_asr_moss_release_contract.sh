#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

python3 - <<'PY'
from pathlib import Path
import importlib.util
import re
import sys
import types

release_path = Path(".github/workflows/release.yml")
runtime_path = Path("crates/bifrost-admin/src/handlers/asr_jobs/moss_joint.rs")
requirements_path = Path("scripts/asr/moss-mlx-requirements.txt")
runner_path = Path("scripts/asr/moss_mlx_runner.py")
patch_path = Path("scripts/asr/mlx-audio-moss-quantized-conv.patch")
packager_path = Path("scripts/ci/package-moss-release-runtime.sh")
packager_test_path = Path("scripts/ci/test-package-moss-release-runtime.sh")
core_guard_path = Path("scripts/ci/check-macos-release-core-payload.sh")
core_guard_test_path = Path("scripts/ci/test-macos-release-core-payload.sh")
ci_path = Path(".github/workflows/ci.yml")

release = release_path.read_text(encoding="utf-8")
runtime = runtime_path.read_text(encoding="utf-8")
requirements = requirements_path.read_text(encoding="utf-8")
runner = runner_path.read_text(encoding="utf-8")
packager = packager_path.read_text(encoding="utf-8")
ci = ci_path.read_text(encoding="utf-8")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"MOSS release contract failed: {message}")


def capture(text: str, pattern: str, label: str) -> str:
    match = re.search(pattern, text, re.MULTILINE | re.DOTALL)
    require(match is not None, f"cannot find {label}")
    return match.group(1)


source_commit = capture(release, r'SOURCE_COMMIT="([0-9a-f]{40})"', "MLX-Audio source commit")
model_commit = capture(release, r'MODEL_COMMIT="([0-9a-f]{40})"', "model snapshot commit")
model_url = capture(runtime, r'const MOSS_MODEL_URL: &str =\s*"([^"]+)";', "model URL")
model_bytes = capture(runtime, r'const MOSS_MODEL_BYTES: u64 = ([0-9_]+);', "model byte length")
model_sha = capture(
    runtime,
    r'const MOSS_MODEL_SHA256: &str =\s*"([0-9a-f]{64})";',
    "model SHA-256",
)
required_block = capture(
    runtime,
    r'const MOSS_MODEL_REQUIRED_FILES: &\[&str\] = &\[(.*?)\];',
    "required model metadata",
)
required_files = set(re.findall(r'"([^"]+)"', required_block))
download_block = capture(
    release,
    r'for file in \\\s*(.*?)\s*; do\s*\n\s*curl --fail',
    "release model metadata download list",
)
download_files = set(download_block.replace("\\", " ").split())

require(source_commit == "64e8416c303fb3b3463dab8eb4ebd78c55a87c1a", "MLX-Audio commit drifted")
require(model_commit in model_url, "runtime model URL and release snapshot disagree")
require(model_url.endswith("/model.safetensors"), "runtime no longer downloads the pinned weight")
require(model_bytes.replace("_", "") == "1258427442", "model byte-length guard drifted")
require(model_sha == "469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190", "model checksum guard drifted")
require(required_files <= download_files, "release archive omits required model metadata")
require("model.safetensors" not in download_files, "runtime archive must not duplicate the separately verified 1.2 GB weight")
require("model weights must be downloaded on demand" in packager, "runtime packager does not reject bundled model weights")

requirements_lines = [
    line.strip()
    for line in requirements.splitlines()
    if line.strip() and not line.lstrip().startswith("#")
]
require(requirements_lines, "pinned Python requirements are empty")
require(all(re.fullmatch(r"[A-Za-z0-9_.-]+==[^=\s]+", line) for line in requirements_lines), "every Python requirement must use an exact version")
require(patch_path.is_file(), "pinned MLX-Audio compatibility patch is missing")
require(f'git -C "$SOURCE_DIR" fetch --depth 1 origin "$SOURCE_COMMIT"' in release, "release does not fetch the pinned source commit")
require('git -C "$SOURCE_DIR" apply --unidiff-zero' in release, "release does not apply the compatibility patch")
require('scripts/ci/package-moss-release-runtime.sh' in release, "release does not use the shared MOSS packager")
require('moss_mlx_runner.py" --self-test' in packager, "shared packager does not smoke-test the runtime")
require("PYTHONNOUSERSITE=1" in packager and "HF_HUB_OFFLINE" in runner, "packaged runtime is not isolated from user/network state")
require("stream_generate(" in runner, "runner does not inspect generation before spending the full token budget")
require("EARLY_PROTOCOL_TOKEN_LIMIT = 256" in runner, "runner early protocol guard drifted")
require("moss_remaining_runtime_budget" in runtime, "runtime no longer applies the end-to-end 0.5x budget")
require("validate_moss_audio_input" in runtime, "runtime no longer rejects wasteful audio before MLX startup")
require("moss_failure_is_non_retryable_for_unchanged_source" in runtime, "deterministic MOSS failures will be retried forever")
require("transcript_is_degenerate" in runner, "runner no longer rejects obvious autoregressive loops")

spec = importlib.util.spec_from_file_location("moss_mlx_runner_contract", runner_path)
require(spec is not None and spec.loader is not None, "cannot load MOSS runner module")
runner_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner_module)
require(
    runner_module.parse_protocol_segments("[0.10][S01]hello[1.20]")
    == [{"start": 0.1, "end": 1.2, "speaker": "S01", "text": "hello"}],
    "runner protocol parser lost timestamps or speaker labels",
)
require(
    runner_module.parse_protocol_segments("[0.10][S01]zero duration[0.10]") == [],
    "runner accepts zero-duration speaker segments",
)
require(
    runner_module.transcript_is_degenerate(
        [{"start": 0.0, "end": 1.0, "speaker": "S01", "text": "嗯" * 100}]
    ),
    "runner accepts an obvious repetitive decoding loop",
)

sample_utils = types.ModuleType("mlx_lm.sample_utils")
sample_utils.make_sampler = lambda _temperature: object()
mlx_lm = types.ModuleType("mlx_lm")
mlx_lm.sample_utils = sample_utils
sys.modules["mlx_lm"] = mlx_lm
sys.modules["mlx_lm.sample_utils"] = sample_utils

class FakeTokenizer:
    def __init__(self, text):
        self.text = text

    def decode(self, _tokens, **_kwargs):
        return self.text

class FakeModel:
    def __init__(self, text, token_count):
        self._tokenizer = FakeTokenizer(text)
        self.token_count = token_count

    def stream_generate(self, *_args, **_kwargs):
        for token in range(self.token_count):
            yield token, None

valid = runner_module.generate_protocol_segments(
    FakeModel("[0.10][S01]hello[1.20]", 32),
    Path("fixture.wav"),
    max_tokens=100,
    prompt="protocol",
)
require(valid[0]["speaker"] == "S01", "valid streaming protocol output was rejected")
try:
    runner_module.generate_protocol_segments(
        FakeModel("unstructured output", 512),
        Path("fixture.wav"),
        max_tokens=512,
        prompt="protocol",
    )
except RuntimeError as error:
    require("before 256 generated tokens" in str(error), "invalid protocol did not use bounded early abort")
else:
    require(False, "invalid protocol consumed the unbounded generation path")

asset_template = 'moss-joint-runtime-v${VERSION}-aarch64-apple-darwin.zip'
require(asset_template in release, "release archive name drifted")
require('"{MOSS_RUNTIME_ASSET_STEM}-v{}-aarch64-apple-darwin.zip"' in runtime, "runtime asset name drifted")
require('dist/*.zip' in release and 'dist/*.sha256' in release, "CLI artifact upload omits runtime archive or checksum")
require('-name "*.zip"' in release and '-name "*.sha256"' in release, "release asset collection omits runtime archive or checksum")
require('cat *.sha256 > "bifrost-v${VERSION}-checksums.txt"' in release, "combined release checksum manifest is missing")
require("parse_runtime_checksum_manifest" in runtime, "runtime no longer verifies the release checksum manifest")
require("BIFROST_MOSS_RUNTIME_SHA256 is required with BIFROST_MOSS_RUNTIME_URL" in runtime, "custom runtime URL can bypass checksum verification")
require(packager_test_path.is_file(), "release packaging fixture test is missing")
require(core_guard_path.is_file(), "macOS core package guard is missing")
require(core_guard_test_path.is_file(), "macOS core package guard fixture is missing")
require("Verify macOS CLI core package stays lightweight" in release, "release does not gate the macOS CLI core package")
require('check-macos-release-core-payload.sh "${APP_PATH}"' in release, "release does not gate the macOS desktop app bundle")
require('check-macos-release-core-payload.sh "${DMG_PATH}"' in release, "release does not inspect the final macOS DMG")
require("Release Workflow Contract (MOSS macOS)" in ci, "PR CI does not gate the MOSS release workflow")
require("test-package-moss-release-runtime.sh" in ci, "PR CI does not execute the shared MOSS packager")
require("test-macos-release-core-payload.sh" in ci, "PR CI does not exercise the macOS core package guard")
require(ci.count("Verify macOS CLI binary stays lightweight") == 2, "PR CI does not gate both macOS CLI architectures")
require(ci.count('check-macos-release-core-payload.sh "${DMG_PATH}"') >= 1, "PR CI does not inspect its actual macOS DMG")

print(
    "PASS: MOSS release/runtime contract is aligned "
    f"(source={source_commit[:12]}, model={model_commit[:12]}, metadata={len(download_files)})"
)
PY
