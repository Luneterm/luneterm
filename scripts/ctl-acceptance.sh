#!/usr/bin/env bash
# Words in single quotes are meant as written; `ls` lists names this app made.
# shellcheck disable=SC2016,SC2012
# Acceptance run of `luneterm ctl` (Protocol 1 with the --any-origin amendment) against a real
# Luneterm: it starts scratch instances of $LUNETERM_BIN on a config directory made by mktemp,
# drives them with `ctl` only, and checks every answer and exit code exactly. It never types into a
# tab or reads one. Windows open and close while it runs, and `tab focus` asks the OS to bring them
# to the front.
#
#   LUNETERM_BIN=target/release/luneterm scripts/ctl-acceptance.sh
#
# macOS, a Unix with a desktop, or Windows: Git Bash in the logged-on desktop session. Needs
# python3 (`python` on Windows) for the check of a program's words and environment. On Windows the
# control pipes of one user are one namespace, so no other Luneterm of that user may be running;
# the script stops before starting anything if it reaches one.
#
# Exit 0: every check passed (the scratch directory is removed). 1: a check failed (the directory
# is kept and named). 2: it could not run.
set -u -o pipefail

given=${LUNETERM_BIN:-}
# Inside a Luneterm tab these are inherited and name that instance: none of it is used.
unset LUNETERM_BIN LUNETERM_INSTANCE LUNETERM_TAB_ID

die() {
    echo "ctl-acceptance: $*" >&2
    exit 2
}

win=0
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*)
    win=1
    # Words go to luneterm.exe as written; paths are converted here, by `native`.
    export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
    ;;
esac

# A path as the app writes and reads it.
native() {
    if [ "$win" = 1 ]; then cygpath -l -w "$1"; else printf '%s' "$1"; fi
}

# Text as the app's JSON writer puts it between quotes (no control characters are passed here).
json() {
    printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

repeat() { # <count> <character>
    head -c "$1" /dev/zero | tr '\0' "$2"
}

[ -n "$given" ] || die "set LUNETERM_BIN to the luneterm executable to test"
if [ "$win" = 1 ]; then given=$(cygpath -u "$given"); fi
[ -f "$given" ] && [ -x "$given" ] || die "LUNETERM_BIN is not an executable file: $given"
BIN=$(cd "$(dirname "$given")" && pwd)/$(basename "$given")

PY=
for candidate in python3 python; do
    found=$(command -v "$candidate") || continue
    real=$("$found" -I -c 'import sys; print(sys.executable)' 2>/dev/null | tr -d '\r') || continue
    [ -n "$real" ] && PY=$real && break
done
[ -n "$PY" ] || die "no working python3 or python on PATH"

version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' \
    "$(dirname "$0")/../Cargo.toml" 2>/dev/null | head -n 1)
version=${LUNETERM_EXPECT_VERSION:-$version}
[ -n "$version" ] || die "no version to expect: set LUNETERM_EXPECT_VERSION"

ROOT=$(mktemp -d) || die "mktemp -d failed"
[ -d "$ROOT" ] || die "mktemp -d made no directory"
PIDS=
PASS=0
FAIL=0

alive() { kill -0 "$1" 2>/dev/null; }

# Ends an instance this run started, at once.
stop_app() {
    if alive "$1"; then
        if [ "$win" = 1 ]; then
            taskkill /F /PID "$(cat "/proc/$1/winpid")" >/dev/null 2>&1
        else
            kill -9 "$1" 2>/dev/null
        fi
    fi
    wait "$1" 2>/dev/null
}

cleanup() {
    status=$?
    for pid in $PIDS; do
        if alive "$pid"; then stop_app "$pid"; fi
    done
    if [ "$status" = 0 ]; then
        # Children of a closed ConPTY can hold their directory for a moment.
        rm -rf "$ROOT" 2>/dev/null || { sleep 2 && rm -rf "$ROOT"; }
    else
        echo "ctl-acceptance: scratch directory kept: $ROOT" >&2
    fi
}
trap cleanup EXIT

# The config directory is the scratch directory itself (the socket path must stay short).
if [ "$win" = 0 ] && [ $((${#ROOT} + 36)) -gt 103 ]; then
    die "the scratch path is too long for a socket; set TMPDIR to a shorter directory"
fi
CONFIG=$(native "$ROOT")
export LUNETERM_CONFIG_DIR=$CONFIG
mkdir "$ROOT/home" "$ROOT/w1" "$ROOT/w2" "$ROOT/w3" "$ROOT/w4" || die "cannot make directories"
: >"$ROOT/file"
W1=$(native "$ROOT/w1")
W2=$(native "$ROOT/w2")
W3=$(native "$ROOT/w3")
W4=$(native "$ROOT/w4")
MISSING_DIR=$(native "$ROOT/missing")
A_FILE=$(native "$ROOT/file")

# The program a program tab runs, and the script that keeps it running.
if [ "$win" = 1 ]; then
    SH=$(cygpath -l -w "$(command -v sh).exe")
    NAP='/usr/bin/sleep 600'
else
    SH=/bin/sh
    NAP='sleep 600'
fi

# Three of the user's shell tabs for the next start.
seed_workspace() {
    printf '{"version":1,"active":0,"tabs":[{"cwd":"%s"},{"cwd":"%s"},{"cwd":"%s"}]}\n' \
        "$(json "$W1")" "$(json "$W2")" "$(json "$W3")" >"$ROOT/workspace.json"
}

start_app() { # <log name>; sets APP
    if [ "$win" = 1 ]; then
        "$BIN" >"$ROOT/$1.log" 2>&1 &
    else
        HOME="$ROOT/home" SHELL=/bin/sh "$BIN" >"$ROOT/$1.log" 2>&1 &
    fi
    APP=$!
    PIDS="$PIDS $APP"
}

# ----------------------------------------------------------------------------- calls and checks

INSTANCE=
CODE=0

# One `ctl` run against $INSTANCE (empty counts as unset): exit code, stdout, stderr.
ctl() {
    LUNETERM_INSTANCE=$INSTANCE "$BIN" ctl "$@" >"$ROOT/out" 2>"$ROOT/err" </dev/null
    CODE=$?
}

pass() {
    PASS=$((PASS + 1))
    echo "ok   $1"
}

fail() {
    FAIL=$((FAIL + 1))
    echo "FAIL $1"
    if [ -n "${2:-}" ]; then echo "     $2"; fi
}

check() { # <description> <command...>
    local desc=$1
    shift
    if "$@"; then pass "$desc"; else fail "$desc"; fi
}

got() {
    printf 'exit %s, stdout %q, stderr %q' "$CODE" "$(head -c 600 "$ROOT/out")" \
        "$(head -c 600 "$ROOT/err")"
}

# Done: exit 0, stdout exactly the line and a newline, nothing on stderr.
expect_line() { # <description> <line> <words...>
    local desc=$1 line=$2
    shift 2
    ctl "$@"
    printf '%s\n' "$line" >"$ROOT/want"
    if [ "$CODE" = 0 ] && cmp -s "$ROOT/want" "$ROOT/out" && [ ! -s "$ROOT/err" ]; then
        pass "$desc"
    else
        fail "$desc" "want exit 0 and $line; got $(got)"
    fi
}

# Refused: the exit code, nothing on stdout, exactly one `luneterm:` line on stderr.
expect_refused() { # <description> <code> <reason> <words...>
    local desc=$1 code=$2 reason=$3
    shift 3
    ctl "$@"
    printf 'luneterm: %s\n' "$reason" >"$ROOT/want"
    if [ "$CODE" = "$code" ] && [ ! -s "$ROOT/out" ] && cmp -s "$ROOT/want" "$ROOT/err"; then
        pass "$desc"
    else
        fail "$desc" "want exit $code and 'luneterm: $reason'; got $(got)"
    fi
}

# `tab new`: exit 0 and exactly {"id":<n>}; sets NEW.
expect_new() { # <description> <words after `tab new`...>
    local desc=$1
    shift
    ctl tab new "$@"
    NEW=$(sed -n 's/^{"id":\([0-9][0-9]*\)}$/\1/p' "$ROOT/out")
    printf '{"id":%s}\n' "$NEW" >"$ROOT/want"
    if [ "$CODE" = 0 ] && [ -n "$NEW" ] && cmp -s "$ROOT/want" "$ROOT/out" &&
        [ ! -s "$ROOT/err" ]; then
        pass "$desc (id $NEW)"
    else
        NEW=0
        fail "$desc" "want exit 0 and {\"id\":<n>}; got $(got)"
    fi
}

# `tab list` into $ROOT/rows, one tab a line, each pid written P; $ROOT/pids keeps "<id> <pid>".
read_list() {
    ctl tab list
    : >"$ROOT/rows"
    : >"$ROOT/pids"
    [ "$CODE" = 0 ] && [ ! -s "$ROOT/err" ] && [ "$(wc -l <"$ROOT/out" | tr -d ' ')" = 1 ] || return 1
    awk '{ gsub(/\[\{"id":/, "[\n{\"id\":"); gsub(/\},\{"id":/, "}\n{\"id\":"); sub(/\]\}$/, "\n]}"); print }' \
        "$ROOT/out" >"$ROOT/split"
    [ "$(head -n 1 "$ROOT/split")" = "{\"instance\":\"$TOKEN\",\"tabs\":[" ] || return 1
    [ "$(tail -n 1 "$ROOT/split")" = ']}' ] || return 1
    sed '1d;$d' "$ROOT/split" >"$ROOT/raw"
    sed -n 's/^{"id":\([0-9]*\),.*"pid":\([0-9]*\),.*/\1 \2/p' "$ROOT/raw" >"$ROOT/pids"
    sed 's/"pid":[0-9][0-9]*,/"pid":P,/' "$ROOT/raw" >"$ROOT/rows"
}

ids() { sed 's/^{"id":\([0-9]*\),.*/\1/' "$ROOT/rows" | tr '\n' ' ' | sed 's/ $//'; }
row() { grep "^{\"id\":$1," "$ROOT/rows"; }
pid_of() { sed -n "s/^$1 //p" "$ROOT/pids"; }
active_ids() { grep '"active":true' "$ROOT/rows" | sed 's/^{"id":\([0-9]*\),.*/\1/' | tr '\n' ' ' | sed 's/ $//'; }

expect_order() { # <description> <ids in order...>
    local desc=$1
    shift
    if read_list && [ "$(ids)" = "$*" ]; then
        pass "$desc"
    else
        fail "$desc" "want tabs '$*'; got '$(ids)' ($(got))"
    fi
}

expect_active() { # <description> <id>
    if read_list && [ "$(active_ids)" = "$2" ]; then
        pass "$1"
    else
        fail "$1" "want active tab $2; got '$(active_ids)'"
    fi
}

# A control tab's row, whole.
control_row() { # <id> <group or null> <title> <cwd> <active>
    local group=null
    [ "$2" = null ] || group="\"$(json "$2")\""
    printf '{"id":%s,"origin":"control","group":%s,"title":"%s","cwd":"%s","active":%s,"pid":P,"input_idle_ms":null}' \
        "$1" "$group" "$(json "$3")" "$(json "$4")" "$5"
}

expect_row() { # <description> <id> <the row, whole>
    if read_list && [ "$(row "$2")" = "$3" ]; then
        pass "$1"
    else
        fail "$1" "want $3; got $(row "$2")"
    fi
}

# A shell tab's row: its title and directory are the shell's to say.
expect_shell_row() { # <description> <id> <origin> <group or null> <active>
    local text='"([^"\\]|\\.)*"' group=null
    [ "$4" = null ] || group="\"$4\""
    if read_list && row "$2" | grep -Eq "^\\{\"id\":$2,\"origin\":\"$3\",\"group\":$group,\"title\":$text,\"cwd\":(null|$text),\"active\":$5,\"pid\":P,\"input_idle_ms\":null\\}\$"; then
        pass "$1"
    else
        fail "$1" "want a $3 shell tab $2, group $4, active $5; got $(row "$2")"
    fi
}

wait_for() { # <seconds> <command...>
    local tries=$(($1 * 5))
    shift
    while [ "$tries" -gt 0 ]; do
        if "$@"; then return 0; fi
        sleep 0.2
        tries=$((tries - 1))
    done
    return 1
}

gone() { ! alive "$1"; }
process_gone() {
    if [ "$win" = 1 ]; then
        ! tasklist /FI "PID eq $1" /NH 2>/dev/null | grep -qw "$1"
    else
        ! kill -0 "$1" 2>/dev/null
    fi
}
listed() { read_list && row "$1" >/dev/null; }
not_listed() { read_list && ! row "$1" >/dev/null; }
has_tabs() { read_list && [ "$(wc -l <"$ROOT/rows" | tr -d ' ')" = "$1" ]; }

# Waits for `status` to answer for the one instance there is; sets TOKEN.
ready() {
    ctl status
    [ "$CODE" = 0 ] || return 1
    TOKEN=$(sed -n 's/^.*"instance":"\([A-Za-z0-9_-]*\)".*$/\1/p' "$ROOT/out")
}

status_line() { # <token> <on|off>
    printf '{"protocol":1,"version":"%s","instance":"%s","control":"%s"}' "$version" "$1" "$2"
}

NONE='no running Luneterm'
TWO='more than one Luneterm is running; set LUNETERM_INSTANCE'
GONE='the Luneterm that LUNETERM_INSTANCE names cannot be reached'
OFF='tab control is turned off in settings'
NOT_YOURS='the tab was not opened through ctl; --any-origin closes it'
NO_TAB='no such tab'
BAD_COMMAND='unknown command, or the wrong number of arguments'
BAD_NEW='unknown argument to `tab new`'
BAD_CLOSE='`tab close` takes <id> [--any-origin]'
BAD_ID='a tab id is a plain decimal number'
BAD_NAME='a name is empty, too long, has an edge space or has a control character'
TWICE='an option is given twice'
NO_VALUE='an option has no value'

# ------------------------------------------------------------------------------ before a start

echo "ctl-acceptance: $BIN (expects version $version)"
echo "ctl-acceptance: config directory $CONFIG"

# A binary without `ctl` would open a window here and never answer.
"$BIN" ctl status >"$ROOT/out" 2>"$ROOT/err" </dev/null &
probe=$!
PIDS="$PIDS $probe"
wait_for 10 gone "$probe" || die "'ctl status' did not return: is this a luneterm with ctl?"
wait "$probe"
CODE=$?
[ "$CODE" = 4 ] && [ "$(cat "$ROOT/err")" = "luneterm: $NONE" ] ||
    die "expected no Luneterm to be reachable before the start; got $(got)"
pass "no instance: status is 4"
expect_refused "no instance: tab list is 4" 4 "$NONE" tab list

# ------------------------------------------------------------------------------ the sequence

seed_workspace
start_app app1
APP1=$APP
wait_for 60 ready || die "the first instance did not answer in 60 s (see $ROOT/app1.log)"
TOKEN1=$TOKEN
wait_for 30 has_tabs 3 || die "the three saved tabs did not open"

expect_line "status without LUNETERM_INSTANCE" "$(status_line "$TOKEN1" on)" status
printf '%s\n' "$(status_line "$TOKEN1" on)" >"$ROOT/want"
if env -u LUNETERM_INSTANCE "$BIN" ctl status 2>"$ROOT/err" | cmp -s - "$ROOT/want"; then
    pass "status with LUNETERM_INSTANCE unset"
else
    fail "status with LUNETERM_INSTANCE unset"
fi
INSTANCE=$TOKEN1
expect_line "status with LUNETERM_INSTANCE" "$(status_line "$TOKEN1" on)" status

read_list || die "tab list failed: $(got)"
# shellcheck disable=SC2046 # ids are numbers
set -- $(ids)
U1=$1
U2=$2
U3=$3
expect_shell_row "user tab 1: user, active" "$U1" user null true
expect_shell_row "user tab 2: user" "$U2" user null false
expect_shell_row "user tab 3: user" "$U3" user null false

# orch's sequence.
expect_new "tab new: a program in a new group, titled" \
    --cwd "$W4" --group "orch: x" --title x -- "$SH" -c "$NAP"
N1=$NEW
expect_row "the new tab: control, grouped, titled, cwd, in the background" "$N1" \
    "$(control_row "$N1" "orch: x" x "$W4" false)"
expect_order "it is at the end" "$U1" "$U2" "$U3" "$N1"
expect_active "tab new did not move the active tab" "$U1"
N1_PID=$(pid_of "$N1")
check "tab list gives its pid" [ -n "$N1_PID" ]

expect_new "tab new: the user's shell, ungrouped" --cwd "$W1"
LOOSE=$NEW
expect_new "tab new --group: after the group's last member" --cwd "$W2" --group "orch: x"
N2=$NEW
expect_order "order: group members adjacent" "$U1" "$U2" "$U3" "$N1" "$N2" "$LOOSE"
expect_shell_row "the grouped shell tab" "$N2" control "orch: x" false

expect_line "tab focus" "{\"focused\":$N1}" tab focus "$N1"
expect_active "the focused tab is active" "$N1"
expect_row "and its row says so" "$N1" "$(control_row "$N1" "orch: x" x "$W4" true)"

expect_line "tab group: a user tab joins" "{\"id\":$U2,\"group\":\"orch: x\"}" tab group "$U2" "orch: x"
expect_order "it goes after the group's last member" "$U1" "$U3" "$N1" "$N2" "$U2" "$LOOSE"
expect_shell_row "and is still the user's tab" "$U2" user "orch: x" false
expect_line "tab group: again is the same" "{\"id\":$U2,\"group\":\"orch: x\"}" tab group "$U2" "orch: x"
expect_line "tab ungroup" "{\"id\":$U2,\"group\":null}" tab ungroup "$U2"
expect_shell_row "it has no group" "$U2" user null false
expect_line "tab group: a new name makes a group" "{\"id\":$U1,\"group\":\"other\"}" tab group "$U1" other
expect_shell_row "in the new group" "$U1" user other false
expect_line "tab ungroup: the new group goes" "{\"id\":$U1,\"group\":null}" tab ungroup "$U1"
expect_line "tab ungroup: no group is fine" "{\"id\":$U1,\"group\":null}" tab ungroup "$U1"
expect_active "grouping did not move the active tab" "$N1"
expect_order "order after the group changes" "$U1" "$U3" "$N1" "$N2" "$U2" "$LOOSE"

expect_line "tab close: a control tab" "{\"closed\":$N1}" tab close "$N1"
expect_order "it is gone" "$U1" "$U3" "$N2" "$U2" "$LOOSE"
check "its program ended" wait_for 10 process_gone "${N1_PID:-0}"
expect_refused "a closed tab is no tab" 5 "$NO_TAB" tab close "$N1"

expect_refused "tab close: a user tab is refused" 6 "$NOT_YOURS" tab close "$U1"
check "the refused tab is still there" listed "$U1"
expect_line "tab close <id> --any-origin: a user tab" "{\"closed\":$U1}" tab close "$U1" --any-origin
check "and is gone" wait_for 5 not_listed "$U1"
expect_refused "the second user tab is refused too" 6 "$NOT_YOURS" tab close "$U3"
expect_line "tab close --any-origin <id>" "{\"closed\":$U3}" tab close --any-origin "$U3"
check "and is gone" wait_for 5 not_listed "$U3"
expect_line "--any-origin on a control tab" "{\"closed\":$LOOSE}" tab close "$LOOSE" --any-origin
expect_order "what is left" "$N2" "$U2"

NOBODY=999999
expect_refused "unknown id: close" 5 "$NO_TAB" tab close "$NOBODY"
expect_refused "unknown id: close --any-origin after" 5 "$NO_TAB" tab close "$NOBODY" --any-origin
expect_refused "unknown id: close --any-origin before" 5 "$NO_TAB" tab close --any-origin "$NOBODY"
expect_refused "unknown id: focus" 5 "$NO_TAB" tab focus "$NOBODY"
expect_refused "unknown id: group" 5 "$NO_TAB" tab group "$NOBODY" x
expect_refused "unknown id: ungroup" 5 "$NO_TAB" tab ungroup "$NOBODY"

# ------------------------------------------------------------------------------ a program tab

# The program writes the words and the variables it got to a file, and exits.
CHECKER='import os, sys
out = sys.argv[1]
names = ("LUNETERM_BIN", "LUNETERM_INSTANCE", "LUNETERM_TAB_ID")
items = sys.argv[2:] + [os.environ.get(n, "<unset>") for n in names] + [os.getcwd()]
with open(out + ".part", "wb") as f:
    f.write(b"".join(i.encode("utf-8", "surrogateescape") + b"\0" for i in items))
os.replace(out + ".part", out)'
words=('two words' '$(echo injected)' '$HOME' ';ls' '--' '-c' '--focus' 'a"quote' 'back\slash' ''
    'ünïcødé' $'line one\nline two' '%PATH%' '*' '&& echo' '`id`')
read_list || die "tab list failed: $(got)"
BEFORE=$(active_ids)
expect_new "tab new: a program with awkward words" \
    --cwd "$W3" -- "$PY" -I -c "$CHECKER" "$(native "$ROOT/argv")" "${words[@]}"
PROG=$NEW
if wait_for 20 [ -f "$ROOT/argv" ]; then
    seen=()
    while IFS= read -r -d '' item; do seen+=("$item"); done <"$ROOT/argv"
    count=${#words[@]}
    same=1
    [ "${#seen[@]}" = $((count + 4)) ] || same=0
    for ((i = 0; i < count; i++)); do
        [ "${seen[$i]-<missing>}" = "${words[$i]}" ] || {
            same=0
            printf '     word %s: want %q, got %q\n' "$i" "${words[$i]}" "${seen[$i]-<missing>}"
        }
    done
    check "the program got its words byte for byte" [ "$same" = 1 ]
    want_bin=$(native "$("$PY" -I -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' \
        "$(native "$BIN")" | tr -d '\r')")
    got_bin=${seen[$count]-}
    got_cwd=${seen[$((count + 3))]-}
    want_cwd=$W3
    if [ "$win" = 1 ]; then # case does not name a file here
        want_bin=$(printf '%s' "$want_bin" | tr '[:upper:]' '[:lower:]')
        got_bin=$(printf '%s' "$got_bin" | tr '[:upper:]' '[:lower:]')
        want_cwd=$(printf '%s' "$want_cwd" | tr '[:upper:]' '[:lower:]')
        got_cwd=$(printf '%s' "$got_cwd" | tr '[:upper:]' '[:lower:]')
    else # macOS gives a directory under /var as /private/var
        want_cwd=$(cd "$ROOT/w3" && pwd -P)
        got_cwd=$(cd "$got_cwd" 2>/dev/null && pwd -P)
    fi
    check "LUNETERM_BIN is the app's executable ($got_bin)" [ "$got_bin" = "$want_bin" ]
    check "LUNETERM_INSTANCE is the instance" [ "${seen[$((count + 1))]-}" = "$TOKEN1" ]
    check "LUNETERM_TAB_ID is the new tab's id" [ "${seen[$((count + 2))]-}" = "$PROG" ]
    check "it started in --cwd" [ "$got_cwd" = "$want_cwd" ]
else
    fail "the program ran and wrote its file"
fi
check "the tab closed when the program exited" wait_for 10 not_listed "$PROG"
expect_active "the program tab came and went in the background" "$BEFORE"

# ------------------------------------------------------------------------------ at the limits

NAME64=$(repeat 64 g)
TITLE128=$(repeat 128 t)
expect_new "a 64-character group name and a 128-character title" \
    --cwd "$W1" --group "$NAME64" --title "$TITLE128" -- "$SH" -c "$NAP"
AT=$NEW
expect_row "kept whole" "$AT" "$(control_row "$AT" "$NAME64" "$TITLE128" "$W1" false)"
expect_line "tab close" "{\"closed\":$AT}" tab close "$AT"

many=()
for ((i = 4; i <= 256; i++)); do many+=("w$i"); done # with <sh> -c <script>: 256 words
expect_new "256 words after --" --cwd "$W1" -- "$SH" -c "$NAP" "${many[@]}"
expect_line "tab close" "{\"closed\":$NEW}" tab close "$NEW"
many+=(one-more)

if [ "$win" = 0 ]; then # a Windows command line holds under 32 KiB
    room=$((65536 - ${#SH} - 2 - ${#NAP}))
    BIG=$(repeat "$room" a)
    expect_new "64 KiB after --" --cwd "$W1" -- "$SH" -c "$NAP" "$BIG"
    expect_line "tab close" "{\"closed\":$NEW}" tab close "$NEW"
else
    echo "skip 64 KiB after -- (the command line limit of Windows is lower)"
fi

# ------------------------------------------------------------------------------ usage errors

read_list || die "tab list failed: $(got)"
cp "$ROOT/rows" "$ROOT/rows.before"

usage() { # <reason> <words...>
    local reason=$1 shown
    shift
    shown=$(printf '%q ' "$@")
    expect_refused "usage: ${shown:0:90}" 2 "$reason" "$@"
}
usage "$BAD_COMMAND"
usage "$BAD_COMMAND" bogus
usage "$BAD_COMMAND" status extra
usage "$BAD_COMMAND" tab
usage "$BAD_COMMAND" tab list --all
usage "$BAD_COMMAND" tab focus
usage "$BAD_COMMAND" tab group "$U2"
usage "$BAD_COMMAND" tab ungroup "$U2" extra
usage '`tab new` needs --cwd <dir>' tab new
usage "$NO_VALUE" tab new --cwd
usage "$NO_VALUE" tab new --cwd --focus
usage "$NO_VALUE" tab new --cwd "$W1" --group
usage "$TWICE" tab new --cwd "$W1" --cwd "$W1"
usage "$TWICE" tab new --cwd "$W1" --group a --group a
usage "$TWICE" tab new --cwd "$W1" --title a --title b
usage "$TWICE" tab new --cwd "$W1" --focus --focus
usage "$BAD_NEW" tab new --cwd "$W1" --bogus
usage "$BAD_NEW" tab new "--cwd=$W1"
usage "$BAD_NEW" tab new --cwd "$W1" extra
usage '`--` needs a program' tab new --cwd "$W1" --
usage '--cwd is not an absolute local path' tab new --cwd relative
usage '--cwd is not an existing directory' tab new --cwd "$MISSING_DIR"
usage '--cwd is not an existing directory' tab new --cwd "$A_FILE"
usage "$BAD_CLOSE" tab close
usage "$BAD_CLOSE" tab close "$U2" --any-origin --any-origin
usage "$BAD_CLOSE" tab close "$U2" --force
usage "$BAD_ID" tab close --any-origin
usage "$BAD_ID" tab focus 0"$U2"
usage "$BAD_ID" tab focus -1
usage "$BAD_ID" tab focus x
usage "$BAD_ID" tab focus 18446744073709551616
usage "$BAD_NAME" tab group "$U2" ''
usage "$BAD_NAME" tab group "$U2" "g$NAME64"
usage "$BAD_NAME" tab group "$U2" $'a\tb'
usage "$BAD_NAME" tab group "$U2" ' edge'
usage "$BAD_NAME" tab group "$U2" $'a\xe2\x80\xaeb'
usage "$BAD_NAME" tab new --cwd "$W1" --group "g$NAME64"
usage "$BAD_NAME" tab new --cwd "$W1" --title "t$TITLE128"
usage 'more than 256 arguments after `--`' tab new --cwd "$W1" -- "$SH" -c "$NAP" "${many[@]}"
if [ "$win" = 0 ]; then
    usage 'more than 64 KiB of arguments after `--`' tab new --cwd "$W1" -- "$SH" -c "$NAP" "${BIG}a"
    usage 'an argument is not valid Unicode' tab group "$U2" $'\xff'
fi
check "no usage error changed a tab" eval 'read_list && cmp -s "$ROOT/rows" "$ROOT/rows.before"'

expect_new "tab new --focus" --cwd "$W2" --focus
FOCUSED=$NEW
expect_active "the new tab is active" "$FOCUSED"
expect_order "and is at the end" "$N2" "$U2" "$FOCUSED"

# ------------------------------------------------------------------------------ two instances

start_app app2
APP2=$APP
INSTANCE=
two_running() {
    ctl status
    [ "$CODE" = 4 ] && [ "$(cat "$ROOT/err")" = "luneterm: $TWO" ]
}
wait_for 60 two_running || die "the second instance did not show up (see $ROOT/app2.log): $(got)"

other_tokens() {
    if [ "$win" = 1 ]; then
        powershell.exe -NoProfile -NonInteractive -Command \
            '[IO.Directory]::GetFiles("\\.\pipe\") | ForEach-Object { $_ }' 2>/dev/null |
            tr -d '\r' | sed -n 's/^.*luneterm-ctl-\([A-Za-z0-9_-]*\)$/\1/p'
    else
        ls "$ROOT/run" | sed -n 's/^\([A-Za-z0-9_-]*\)\.sock$/\1/p'
    fi | grep -vx -- "$TOKEN1"
}
TOKEN2=$(other_tokens)
case "$TOKEN2" in
'' | *$'\n'*) die "expected one other instance's token" ;;
esac

expect_refused "two instances, no LUNETERM_INSTANCE: status is 4" 4 "$TWO" status
expect_refused "two instances: tab list is 4" 4 "$TWO" tab list
expect_refused "two instances: tab new is 4" 4 "$TWO" tab new --cwd "$W1"
INSTANCE=$TOKEN1
expect_line "LUNETERM_INSTANCE reaches the first" "$(status_line "$TOKEN1" on)" status
INSTANCE=$TOKEN2
expect_line "LUNETERM_INSTANCE reaches the second" "$(status_line "$TOKEN2" on)" status
INSTANCE=AAAAAAAAAAAAAAAAAAAAAA
expect_refused "an instance that is not there is 4" 4 "$GONE" status
INSTANCE=short
expect_refused "a malformed LUNETERM_INSTANCE is 4" 4 'LUNETERM_INSTANCE is not an instance token' status

stop_app "$APP2" # killed: on Unix its socket stays behind
INSTANCE=
check "the first is found again once the second is gone" wait_for 10 eval \
    'ctl status; [ "$CODE" = 0 ] && grep -qF "\"instance\":\"$TOKEN1\"" "$ROOT/out"'
INSTANCE=$TOKEN2
expect_refused "the gone instance is 4" 4 "$GONE" tab list

# ------------------------------------------------------------------------------ the last tab

INSTANCE=$TOKEN1
TOKEN=$TOKEN1
read_list || die "tab list failed: $(got)"
for id in $(ids); do
    expect_line "tab close --any-origin: tab $id" "{\"closed\":$id}" tab close "$id" --any-origin
done
check "closing the last tab quits Luneterm" wait_for 20 gone "$APP1"
if gone "$APP1"; then
    wait "$APP1"
    status=$?
    check "it exited 0 (got $status)" [ "$status" = 0 ]
fi
if [ "$win" = 0 ]; then
    check "it removed its socket" [ ! -e "$ROOT/run/$TOKEN1.sock" ]
fi
expect_refused "a quit instance is 4" 4 "$GONE" status
INSTANCE=
expect_refused "no instance left: 4" 4 "$NONE" status

# ------------------------------------------------------------------------------ control off

printf '{"version":1,"tab_control":false}\n' >"$ROOT/settings.json"
seed_workspace
start_app app3
APP3=$APP
wait_for 60 ready || die "the instance with control off did not answer (see $ROOT/app3.log)"
TOKEN3=$TOKEN
check "a restart has a new instance token" [ "$TOKEN3" != "$TOKEN1" ]
expect_line "off: status works and says off" "$(status_line "$TOKEN3" off)" status
INSTANCE=$TOKEN3
expect_line "off: status with LUNETERM_INSTANCE" "$(status_line "$TOKEN3" off)" status
expect_refused "off: tab list" 3 "$OFF" tab list
expect_refused "off: tab new" 3 "$OFF" tab new --cwd "$W1"
expect_refused "off: tab new with a program" 3 "$OFF" tab new --cwd "$W1" --group g --focus -- "$SH" -c "$NAP"
expect_refused "off: tab close" 3 "$OFF" tab close 1
expect_refused "off: tab close --any-origin" 3 "$OFF" tab close 1 --any-origin
expect_refused "off: tab close, unknown id" 3 "$OFF" tab close "$NOBODY"
expect_refused "off: tab focus" 3 "$OFF" tab focus 1
expect_refused "off: tab group" 3 "$OFF" tab group 1 g
expect_refused "off: tab ungroup" 3 "$OFF" tab ungroup 1
expect_refused "off: a usage error is still 2" 2 "$BAD_COMMAND" tab
check "off: the app is still running" alive "$APP3"
stop_app "$APP3"
INSTANCE=
expect_refused "after the last instance: 4" 4 "$NONE" status

# ------------------------------------------------------------------------------ the result

echo "ctl-acceptance: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
