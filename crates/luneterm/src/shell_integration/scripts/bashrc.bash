# Source: Lunegit's TerminalShellIntegrator.cs (integration version 5), identity changed to Luneterm.
# Luneterm shell integration — do not edit. Emulates login-shell startup sourcing, then installs
# an OSC 7 cwd emitter into PROMPT_COMMAND (appended, never overwriting the user's value).
if [ -r /etc/profile ]; then . /etc/profile; fi
for _luneterm_f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
	if [ -r "$_luneterm_f" ]; then . "$_luneterm_f"; break; fi
done
unset _luneterm_f
if [ -r "$HOME/.bashrc" ]; then . "$HOME/.bashrc"; fi

# Percent-encode $PWD byte-by-byte (LC_ALL=C so multibyte/UTF-8 encodes per byte), keeping only
# RFC 3986 unreserved chars + '/' — spec-compliant OSC 7 and the exact inverse of the scanner's
# percent-decode, so a dir name with a literal '%'/spaces/unicode round-trips. Mirrors vte.sh.
_luneterm_osc7() {
	local LC_ALL=C
	local str=$PWD safe
	printf '\033]7;file://%s' "${HOSTNAME:-}"
	while [ -n "$str" ]; do
		safe=${str%%[!a-zA-Z0-9._~/-]*}
		printf '%s' "$safe"
		str=${str#"$safe"}
		if [ -n "$str" ]; then
			printf '%%%02X' "'$str"
			str=${str#?}
		fi
	done
	printf '\033\\'
}
case ";${PROMPT_COMMAND};" in
	*";_luneterm_osc7;"*|*"_luneterm_osc7"*) ;;
	*) PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}_luneterm_osc7" ;;
esac
_luneterm_osc7

# ── OSC 133 command markers ──
# D (command end) + A (prompt start) ride PROMPT_COMMAND, which bash runs after each command finishes.
# C (command start) needs a pre-execution hook, which bash does not have natively. Precedence, in order,
# NEVER clobbering a hook we do not own:
#   1. bash-preexec loaded  → append to preexec_functions (its supported extension point);
#   2. no DEBUG trap set    → install ours;
#   3. a foreign DEBUG trap → emit nothing. Bash then reports command END only and the border simply
#      never animates in this shell. That degrade is deliberate.
# The _luneterm_at_prompt flag is what makes the DEBUG trap fire exactly ONCE per user command line:
# without it the trap fires for every simple command, including each stage of a pipeline and each
# command inside PROMPT_COMMAND, producing a storm of C markers.
_luneterm_osc133_prompt() {
	printf '\033]133;D\033\\\033]133;A\033\\'
	_luneterm_at_prompt=1
}
_luneterm_osc133_preexec() {
	if [ "${_luneterm_at_prompt:-0}" = "1" ]; then
		_luneterm_at_prompt=0
		printf '\033]133;C\033\\'
	fi
}
case ";${PROMPT_COMMAND};" in
	*"_luneterm_osc133_prompt"*) ;;
	*) PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}_luneterm_osc133_prompt" ;;
esac
# bash-preexec detection: its own public markers (bash_preexec_imported / the legacy __bp_imported)
# are set at IMPORT time, but preexec_functions itself may still be a declared-but-empty array at
# that point (bash-preexec installs its DEBUG trap lazily, on the first PROMPT_COMMAND run) — and
# on bash's array-parameter-expansion semantics, "${preexec_functions+x}" alone tests element 0, so
# an empty array reads as unset. Check the import markers FIRST so a typical bash-preexec user
# (iTerm2 shell integration, several prompt frameworks) is correctly detected even before its array
# is populated; keep the array check as a fallback for anything that only sets the array.
if [ -n "${bash_preexec_imported:-${__bp_imported:-}}" ] || [ -n "${preexec_functions+x}" ]; then
	case " ${preexec_functions[*]} " in
		*" _luneterm_osc133_preexec "*) ;;
		*) preexec_functions+=(_luneterm_osc133_preexec) ;;
	esac
elif [ -z "$(trap -p DEBUG)" ]; then
	trap '_luneterm_osc133_preexec' DEBUG
fi
# Deliberately 0, not 1: this is the LAST statement the rc file runs before the first real prompt.
# Setting it to 1 here would make the very first PROMPT_COMMAND execution (which runs immediately
# after sourcing finishes) fire the DEBUG trap with the flag already armed, emitting a spurious
# 133;C before the first real 133;D/A — a startup flash. _luneterm_osc133_prompt sets it to 1 itself
# on that first prompt, which is the correct moment.
_luneterm_at_prompt=0
