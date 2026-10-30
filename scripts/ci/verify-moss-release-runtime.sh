#!/usr/bin/env bash

set -euo pipefail

ARCHIVE="${1:?usage: verify-moss-release-runtime.sh <archive> <requirements>}"
REQUIREMENTS="${2:?usage: verify-moss-release-runtime.sh <archive> <requirements>}"
VERIFY_ROOT="$(mktemp -d)"
trap 'rm -rf "$VERIFY_ROOT"' EXIT

(
  cd "$(dirname "$ARCHIVE")"
  shasum -a 256 --check "$(basename "$ARCHIVE").sha256"
)
ditto -x -k "$ARCHIVE" "$VERIFY_ROOT"
RUNTIME_ROOT="$VERIFY_ROOT/moss-joint-runtime"
export PYTHONHOME="$RUNTIME_ROOT/runtime/python"
export PYTHONPATH="$RUNTIME_ROOT/runtime/site-packages"
export PYTHONNOUSERSITE=1
export HF_HUB_OFFLINE=1
export TRANSFORMERS_OFFLINE=1
PYTHON="$PYTHONHOME/bin/python3.12"

"$PYTHON" - "$REQUIREMENTS" "$PYTHONPATH" <<'PY'
from pathlib import Path
import re
import ssl
import sys
import certifi

match = re.search(r"^certifi==([0-9]+\.[0-9]+\.[0-9]+)$", Path(sys.argv[1]).read_text(), re.MULTILINE)
assert match is not None, "missing exact certifi pin"
expected = match.group(1)
assert tuple(map(int, expected.split("."))) >= (2026, 10, 18), "vulnerable certifi pin"
assert certifi.__version__ == expected, (certifi.__version__, expected)
site_packages = Path(sys.argv[2]).resolve()
assert Path(certifi.__file__).resolve().is_relative_to(site_packages), "certifi imported outside archive"
bundle = Path(certifi.where()).resolve()
assert bundle.is_relative_to(site_packages), "CA bundle is outside archive"
assert b"emsign" not in bundle.read_bytes().lower(), "emSign remains in packaged bundle"
# Explicit cafile: validate the shipped bundle independently of host CA overrides.
context = ssl.create_default_context(cafile=str(bundle))
certificates = context.get_ca_certs()
assert certificates, "packaged bundle contains no CA certificates"
assert all("emsign" not in repr(cert).lower() for cert in certificates), "emSign remains in CA subjects"
print(f"PASS: extracted certifi {expected}; {len(certificates)} CAs; emSign absent")
PY

"$PYTHON" "$RUNTIME_ROOT/runtime/moss_mlx_runner.py" --self-test
echo "PASS: extracted runner self-test with HF_HUB_OFFLINE=1 and TRANSFORMERS_OFFLINE=1"
echo "NOT RUN: real transcription requires the separately downloaded 1.2 GB model weights and an audio fixture; neither is bundled"
