# lunegit-oracle

Writes `crates/lune-vt-conformance/goldens/lunegit-input/*.golden` by calling Lunegit's own,
already built, encoders and scanners. Nothing expected is typed by hand: every output column is
what `Lunegit.Infrastructure` returned. Format: `crates/lune-vt-conformance/FORMAT.md`
("Lunegit input goldens").

It loads `Lunegit.Core.dll` and `Lunegit.Infrastructure.dll` by reflection (the encoders and
scanners are `internal`), so it references no Lunegit project and no NuGet package, and it never
builds Lunegit.

## Run

```bash
LUNEGIT_BIN="<Lunegit>/src/Lunegit.Tests/bin/Debug/net10.0" scripts/regen-lunegit-goldens.sh
```

- `LUNEGIT_BIN` (required): a directory holding the two DLLs. The test project's bin dir has them
  once Lunegit's tests have been built. It is read, never written.
- `LUNEGIT_COMMIT` (optional, default `33a1dd4`): recorded in the golden headers as the Lunegit
  commit the DLLs were checked against. The headers also record both DLLs' SHA-256, which is the
  real identity.
- Needs the .NET 10 SDK. Writes only `tools/lunegit-oracle/{bin,obj}` (gitignored) and the goldens
  directory; nothing in the Lunegit tree. A second run must leave `git diff` empty.
- CI never runs it. If a regen changes the goldens, review the diff like any golden change: the
  header shows whether the DLLs changed.

If a Lunegit rename makes a type or method unfindable, the tool stops with a message naming it;
the fallback is the ported Lunegit tests alone, recorded as a gap.
