#!/usr/bin/env bash
# Regenerates crates/lune-vt-conformance/goldens/lunegit-input from Lunegit's own prebuilt
# Lunegit.Core.dll / Lunegit.Infrastructure.dll (tools/lunegit-oracle, by reflection). Needs the
# .NET SDK and LUNEGIT_BIN, the directory holding those two DLLs, for example
#   LUNEGIT_BIN="<Lunegit>/src/Lunegit.Tests/bin/Debug/net10.0" scripts/regen-lunegit-goldens.sh
# Reads LUNEGIT_BIN read-only; writes only tools/lunegit-oracle/{bin,obj} and the goldens dir.
# LUNEGIT_COMMIT (default below) is the Lunegit commit the DLLs were checked against; the header
# also records the DLLs' SHA-256, which is the real identity. Deterministic: a second run leaves
# no git diff. CI never runs this; the goldens are committed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOL="$ROOT/tools/lunegit-oracle"
OUT="$ROOT/crates/lune-vt-conformance/goldens/lunegit-input"
COMMIT="${LUNEGIT_COMMIT:-33a1dd4}"

if ! command -v dotnet > /dev/null 2>&1; then
  echo "regen-lunegit-goldens: 'dotnet' not found on PATH" >&2
  exit 2
fi
if [ -z "${LUNEGIT_BIN:-}" ]; then
  echo "regen-lunegit-goldens: LUNEGIT_BIN is not set (directory holding Lunegit.Core.dll and Lunegit.Infrastructure.dll)" >&2
  exit 2
fi

mkdir -p "$OUT"
dotnet build "$TOOL" -c Release --nologo -v quiet > /dev/null
dotnet "$TOOL/bin/Release/net10.0/lunegit-oracle.dll" --out "$OUT" --commit "$COMMIT"
