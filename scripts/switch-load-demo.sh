#!/bin/sh
# Tab-switch latency under background load, in a real window (plan step 3, pass 2):
#
#   scripts/switch-load-demo.sh [<luneterm binary>] [<mode>]
#
# Starts Luneterm with LUNETERM_TRACE_SWITCH=1 on a throwaway config directory (made with mktemp,
# printed, never your real settings) holding three tabs: the first runs scripts/tui-load.py
# (<mode>: ink, tui, flood; default ink, unthrottled) and the other two are empty `/bin/sh`
# shells. The window opens on an empty tab. Click between the two empty tabs (and to the busy one
# and back) a few times, quit, and keep stderr: its `press:`, `os_press:`, `switch:` and `loop:`
# lines are numbers only (`os_press` is when macOS saw the click, `press` when the app did).
# Set LUNETERM_TRACE_SWITCH_AUTO=<ms> to have the app switch by itself every <ms> as well.
#
# The generator writes generated text and escape sequences only: no network, no files, no real
# work. Binary default: target/release/luneterm (build it with `cargo build --release`).

set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
bin=${1:-$repo/target/release/luneterm}
mode=${2:-ink}
case "$mode" in ink | tui | flood) ;; *)
    echo "usage: $0 [<luneterm binary>] [ink|tui|flood]" >&2
    exit 2
    ;;
esac
[ -x "$bin" ] || {
    echo "$0: no executable at $bin (cargo build --release)" >&2
    exit 2
}
command -v python3 >/dev/null || {
    echo "$0: python3 is needed for scripts/tui-load.py" >&2
    exit 2
}

dir=$(mktemp -d "${TMPDIR:-/tmp}/luneterm-switch-demo.XXXXXX")
echo "config dir: $dir" >&2

# The first tab to start takes the load; every later one is an empty shell.
cat >"$dir/tab.sh" <<EOF
#!/bin/sh
if mkdir "$dir/busy" 2>/dev/null; then
    exec python3 "$repo/scripts/tui-load.py" --mode $mode --fps 0
fi
exec /bin/sh
EOF
chmod 700 "$dir/tab.sh"

cat >"$dir/settings.json" <<EOF
{"version":1,"shell":"$dir/tab.sh","shell_integration":false,"confirm_close":false}
EOF
cat >"$dir/workspace.json" <<EOF
{"version":1,"active":1,"tabs":[{"cwd":"$dir","title":"busy"},{"cwd":"$dir","title":"empty 1"},{"cwd":"$dir","title":"empty 2"}]}
EOF

# When the OS saw each press (macOS), beside the trace's own `press:` lines.
if [ "$(uname)" = Darwin ]; then
    python3 "$repo/scripts/os-press-times.py" &
    sidecar=$!
    trap 'kill "$sidecar" 2>/dev/null' EXIT INT TERM
fi
LUNETERM_CONFIG_DIR=$dir LUNETERM_TRACE_SWITCH=1 "$bin"
