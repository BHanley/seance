#!/usr/bin/env bash
# Draw-cost harness against a THROWAWAY daemon and HOME (never the real ones).
# Usage: scripts/redraw-bench.sh [circles]   (needs target/debug/seance built)
set -euo pipefail
cd "$(dirname "$0")/.."
N=${1:-24}
export CARGO_HOME=${CARGO_HOME:-$HOME/.cargo} RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup}
export PATH="$CARGO_HOME/bin:$PATH"
T=$(mktemp -d /tmp/seance-bench.XXXXXX)
export HOME="$T/home" XDG_RUNTIME_DIR="$T/run" XDG_CONFIG_HOME="$T/home/.config" XDG_STATE_HOME="$T/home/.state"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
if [ -n "${SEANCE_BENCH_MODE:-}" ]; then mkdir -p "$HOME/.config/seance"; echo "{\"layout\":\"$SEANCE_BENCH_MODE\"}" > "$HOME/.config/seance/view.json"; fi
unset SEANCE_SOCKET SEANCE_SESSION SEANCE_PANE_ID SEANCE_WORKSPACE SEANCE_REMOTE
BIN=$PWD/target/debug/seance
"$BIN" daemon >"$T/daemon.log" 2>&1 &
D=$!
trap 'kill -9 $D 2>/dev/null; pkill -f "$T/home" 2>/dev/null; rm -rf "$T"' EXIT
for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/seance.sock" ] && break; sleep 0.2; done
echo "throwaway socket: $XDG_RUNTIME_DIR/seance.sock"
for i in $(seq "$N"); do
  for j in 1 2; do
    "$BIN" ctl --local new --name "p$i-$j" --workspace "circle-$i" --command "sleep 900" >/dev/null
  done
done
export SEANCE_BENCH_PANES=$((N * 2))

exec cargo test --bin seance redraw_bench::${SEANCE_BENCH_TEST:-spinner} -- --ignored --nocapture
