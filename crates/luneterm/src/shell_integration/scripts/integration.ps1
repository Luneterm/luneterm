# Source: Lunegit's TerminalShellIntegrator.cs (integration version 5), identity changed to Luneterm.
# Luneterm shell integration — do not edit. Wraps the existing prompt to emit OSC 9;9 (cwd) plus OSC
# 133 D+A each prompt, and wraps PSReadLine's PSConsoleHostReadLine to emit OSC 133;C the moment a
# command line is accepted. Loaded via -NoExit -Command AFTER the user's profile, so the user's own
# prompt / readline wrapper (if any) is captured and called through, never replaced.
#
# Guards use `Test-Path variable:global:…` rather than a bare `$global:…` read on purpose: a profile
# that runs `Set-StrictMode -Version Latest` makes reading an unset variable a TERMINATING error, so
# the bare-read form killed the whole integration (no cwd, no markers) before it installed anything.
if (-not (Test-Path variable:global:__lunetermPromptWrapped)) {
	$global:__lunetermPromptWrapped = $true
	$global:__lunetermOrigPrompt = $function:prompt

	function global:prompt {
		if ($global:__lunetermOrigPrompt) {
			$out = & $global:__lunetermOrigPrompt
		} else {
			$out = "PS $($ExecutionContext.SessionState.Path.CurrentLocation)> "
		}
		$loc = $ExecutionContext.SessionState.Path.CurrentLocation
		if ($loc -and $loc.Provider.Name -eq 'FileSystem') {
			[Console]::Write([char]27 + ']9;9;' + $loc.ProviderPath + [char]7)
		}
		# OSC 133: the previous command has finished (D) and a new prompt is starting (A). A also
		# doubles as the safety net that clears "busy" if a D is ever missed. Deliberately NO exit
		# code on D, matching zsh/bash: the error/warning tint comes from OSC 9;4.
		[Console]::Write([char]27 + ']133;D' + [char]7)
		[Console]::Write([char]27 + ']133;A' + [char]7)
		return $out
	}
}

# ── OSC 133;C (command start) ──
# PowerShell has no preexec hook, but it does not need one: the console host calls
# PSConsoleHostReadLine (PSReadLine's exported function) to read each command line and then executes
# whatever it returns. Wrapping it and emitting C after the call-through therefore fires exactly once
# per ACCEPTED line, immediately before the command runs — the pwsh equivalent of zsh's preexec and
# bash's DEBUG trap. This is the same mechanism VS Code's own pwsh shell integration uses.
#
# Gated on PSReadLine actually being loaded: without it the host uses its internal reader, so a
# Luneterm-defined PSConsoleHostReadLine would have nothing to call through to and could break line
# editing outright. No PSReadLine → D/A only, exactly the previous behaviour.
#
# The __lunetermReadLineWrapped guard is load-bearing, not cosmetic: running this block twice in one
# session would capture OUR OWN wrapper as the "original" and the call-through would recurse until
# the shell dies of a stack overflow.
if (-not (Test-Path variable:global:__lunetermReadLineWrapped)) {
	try {
		if ((Get-Module -Name PSReadLine) -and (Test-Path function:global:PSConsoleHostReadLine)) {
			$global:__lunetermOrigReadLine = $function:PSConsoleHostReadLine
			$global:__lunetermReadLineWrapped = $true

			function global:PSConsoleHostReadLine {
				# The call-through MUST be the first statement: PSReadLine reads $? as the very first
				# thing it does (its own comment says so — the last command's status feeds history and
				# prediction), and any statement placed above this line would flush it.
				$line = & $global:__lunetermOrigReadLine
				# Skip blank input. Enter on an empty prompt, and Ctrl+C on a typed line, both return
				# an empty string without running anything; a C there would flash the busy outline
				# for a frame before the next prompt's D cleared it. Ctrl+D (PSReadLine's
				# DeleteCharOrExit) is NOT covered by this guard: on an empty buffer it submits a
				# literal "exit" line, so a C IS emitted with no following D/A — that dangling state
				# is cleared by the read-loop's EOF reset (PtyTerminalSession), not by this guard.
				if (-not [string]::IsNullOrWhiteSpace($line)) {
					[Console]::Write([char]27 + ']133;C' + [char]7)
				}
				$line
			}
		}
	} catch {
		# Never break the shell (file-wide rule): any failure just leaves pwsh on the D/A-only path.
	}
}
