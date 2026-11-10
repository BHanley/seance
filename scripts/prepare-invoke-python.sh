#!/usr/bin/env bash
# Build the small Python runtime that Invoke (the phone chat) runs on inside
# Seance.app: a python-build-standalone CPython (via uv) with the standard
# library trimmed, plus `cryptography` and `py-vapid` (web push). Nothing here
# is specific to anyone's setup.
#
#   ./scripts/prepare-invoke-python.sh OUT_DIR
#
# Then build the app with SEANCE_PYTHON_DIR=OUT_DIR (and SEANCE_INVOKE_DIR, see
# bundle-macos.sh).
set -euo pipefail

OUT="${1:?usage: prepare-invoke-python.sh OUT_DIR}"
command -v uv >/dev/null || { echo "uv is needed (https://docs.astral.sh/uv/)" >&2; exit 1; }

uv python install 3.12 >/dev/null
SRC="$(uv python find 3.12 --managed-python)"
ROOT="$(cd "$(dirname "$SRC")/.." && pwd)"
[[ -x "$ROOT/bin/python3" ]] || { echo "no standalone Python found under $ROOT" >&2; exit 1; }

rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$ROOT/." "$OUT/"

LIB="$OUT/lib/python3.12"
# Not needed to run a small web server.
rm -rf "$LIB"/{test,idlelib,tkinter,turtledemo,lib2to3,ensurepip,config-3.12-darwin} \
       "$LIB"/site-packages/{pip,setuptools,wheel,_distutils_hack,pkg_resources} \
       "$LIB"/lib-dynload/_tkinter* "$LIB"/lib-dynload/_test* \
       "$OUT"/share "$OUT"/include "$OUT"/lib/{tcl*,tk*,itcl*,thread*} "$OUT"/lib/*.a
rm -f "$OUT"/bin/{idle3*,pydoc3*,python3-config,python3.12-config,2to3*,pip*}

uv pip install --quiet --python "$OUT/bin/python3" --target "$LIB/site-packages" \
  --only-binary :all: cryptography py-vapid

find "$OUT" -name __pycache__ -type d -prune -exec rm -rf {} +
find "$OUT" -name '*.dist-info' -type d -exec rm -rf {} + 2>/dev/null || true

# It has to start and import what Invoke needs.
"$OUT/bin/python3" -I -c "import ssl, sqlite3, cryptography, py_vapid, http.server; print('ok')" >/dev/null
echo "prepared $OUT ($(du -sh "$OUT" | cut -f1))"
