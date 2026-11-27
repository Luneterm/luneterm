# Updates

Luneterm installs through [lunepack](https://github.com/lune-rs/lunepack) setup files and updates
itself from a signed feed on Cloudflare R2. This page is for the person who publishes: Luneterm's
names and one-time setup, what a release does, and what to do when something goes wrong. How the app
side works is in `docs/architecture.md` "Updates".

Publishing is lunepack's standard flow, `lunepack publish`; **lunepack's `docs/PUBLISHING.md` is the
flow's reference** (the job split, the tokens and their permissions, the environments, rerun and
recovery, the rules for agents) and `docs/CLI.md` "Publishing" the commands. This page does not repeat
them: it gives Luneterm's values and the few things that are Luneterm's own. lunepack is pre-release;
Luneterm pins one revision (`Cargo.toml`, `scripts/check-deps.sh`), and the docs at that revision are
the reference for every `lunepack` command below:
<https://github.com/lune-rs/lunepack/tree/be29be066341c52a1af95f9f6d7d30cabe2cba35/docs>
(`PUBLISHING.md`, `CLI.md`, `FORMAT.md`, `KEY-CEREMONY.md`).

## What users get

| | |
|---|---|
| Installers | `luneterm-<version>-linux-x64-setup` (run `chmod +x` first), `luneterm-<version>-windows-x64-setup.exe`, `luneterm-<version>-macos-arm64-setup.dmg` (a disk image with `Install Luneterm.app`; attached by the `dmg` job a few minutes after the other files, see below) and `luneterm-<version>-macos-arm64-setup.zip` (holds `Luneterm.app`: move it to `/Applications` or `~/Applications`; lunepack adopts it on the first update check). Each has a `.sha256` beside it, attached to the GitHub release. All unsigned (no Developer ID, no Authenticode), so each OS warns on first launch. |
| Installed to | macOS `~/Applications/Luneterm.app` (state in `~/Library/Application Support/my.ijat.luneterm`), Linux `~/.local/share/my.ijat.luneterm` (command link `~/.local/bin/luneterm`), Windows `%LOCALAPPDATA%\Programs\Luneterm`. Per user, no admin rights. |
| Updating | Settings > About: "Check for updates automatically" (on by default: at launch and every 24 h, a found version shows a toast with Update / Dismiss), "Check for updates", and "Restart to update" once an update is downloaded and staged. Restarting closes all shells. |
| Not installed by lunepack | A `cargo run` build or an old portable zip says "Updates need the Luneterm installer" and never touches the network. |
| Rollback | An update that does not confirm itself (`confirm_healthy`, after the first frame and the first shell) within lunepack's health timeout is rolled back to the previous version by lunepack's runtime. |
| Channels | `stable` and `beta`. A copy follows the channel its installer came from: the first run records it as `update_channel` in `settings.json` and keeps it, whatever a later pack says (a beta copy that updates to a stable pack stays on beta). To switch, uninstall and run the other channel's setup file (a setup does not run over an install): the first start after a setup records that setup's channel, whatever `settings.json` (which survives the uninstall) held. Limit: a per-machine install gets no such signal from lunepack and keeps the recorded channel; edit `update_channel` (`stable` or `beta`) in `settings.json` to change it. |

## Where everything lives

| Piece | Where |
|---|---|
| The one config | `packaging/lunepack.toml`: `app_id`, `[root]` (the trust root, only here), `[signers.release]` (the environment variable `LUNEPACK_RELEASE_KEY`), `[update]` (the feed URL and the default channel), `[install]` with `[install.<os>]` (entry and icon per OS), `[windows]` and `[macos]` icons, and `[publish]` (below). It holds public keys and names, never a key or a value. |
| Feed (public) | R2 bucket `izzat-updates`, prefix `luneterm/`, served at `https://updates.izzat.dev/luneterm/` (one shared update host for the user's apps; the Whipspot app uses `whipspot/`). The URL is `[update] feed`, baked into every installed copy by `lunepack pack`. |
| Publisher state (private) | R2 bucket `izzat-updates-publisher`, key `luneterm/lunepack-state.json`, and the pending-deletes record beside it: lunepack's record of what was last signed, so an older copy of a feed cannot be written back. Never public. |
| Feed layout | `luneterm/keys.json`, `luneterm/root-<k>.json` (after a root rotation), `luneterm/feed-stable.json`, `luneterm/feed-beta.json`, and per version `luneterm/<version>/<target>.{pack,manifest.json,from-<old>.delta}`, targets `linux-x86_64`, `windows-x86_64`, `macos-aarch64`. |
| Trust root | The `[root]` table of `packaging/lunepack.toml` and nowhere else. `lunepack pack` writes it into the update config (`lunepack-update.json`) of every pack, and lunepack's updater verifies the feed against that. No Rust source holds a root key. The private root key stays offline with the user. |
| Release signing key | The secret `LUNEPACK_RELEASE_KEY` of three GitHub environments (below), never a repository secret; read only by `.github/workflows/publish-updates.yml`. |
| Glue | `scripts/release-pack.py` (standard library only; tests: `python3 -B -m unittest discover -s scripts/tests`): `check-root`, `pack` (the build inputs from the GitHub release, then `lunepack publish fetch`, `pack`, `setup`) and `attach` (the setup files to the release), `dmg` (the macOS `.dmg` of an already published release: downloads `keys.json`, the root walk, the channel's feed and the macOS manifest and pack from the feed URL, checks the feed, manifest and pack against one another, then `lunepack setup` verifies the chain and writes the `.dmg` and its `.sha256`; needs macOS) and `drop-artifacts` (deletes a run's `lunepack-cli`, `lunepack-cli-macos`, `release` and `setup` artifacts). Everything that publishes is `lunepack publish`. |
| Build inputs | `.github/workflows/release.yml` (Linux, Windows) and `scripts/package-macos.sh` (macOS) build the app and lunepack's runtime for each OS (`scripts/build-lunepack.sh`, `scripts/build-lunepack.ps1`; format: `scripts/make-build-input.py`). |

`[publish]` in `packaging/lunepack.toml` holds the names (`docs/CLI.md` "`[publish]`" has the keys):
prefix `luneterm`, store `r2`, the account id from `R2_ACCOUNT_ID`, the public bucket's token
pair from `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY`, the private bucket's pair from
`R2_PRIVATE_ACCESS_KEY_ID` / `R2_PRIVATE_SECRET_ACCESS_KEY` (the same names as Whipspot's), buckets `izzat-updates` and
`izzat-updates-publisher`, `keep = 10` releases per channel, `deltas = 3`, `feed_days = 90`, channels
`stable` and `beta` with a version that has a pre-release part going to `beta` only, and
`require_targets` the three targets above.

## How a release reaches users

1. Push the tag `v<version>` (or `v<version>-rc.<N>`; the version must equal the workspace version).
   `release.yml` verifies it, builds the Linux and Windows build inputs and creates a draft release
   holding them (`luneterm-<version>-linux-x64-build.tar.gz`, `...-windows-x64-build.zip`, each with
   a `.sha256`). These are not user downloads.
2. On a Mac at the tagged commit: `scripts/package-macos.sh <tag>` builds, assembles and ad-hoc signs
   the app for lunepack and attaches `luneterm-<version>-macos-arm64-build.tar.gz` (+ `.sha256`) to the
   draft. There is no macOS CI. It refuses while `packaging/lunepack.toml` holds the test root
   (`release-pack.py check-root`), so it works only after the key ceremony below.
3. Publish the draft: `gh release edit <tag> --draft=false`. That starts `publish-updates.yml`, which
   follows PUBLISHING.md's job split: `tools-linux` builds the `lunepack` CLI from the pin and
   reports its SHA-256 (no secret); `pack` (environment `release-pack`) takes the build inputs from the
   release, checks each against its `.sha256` and its `BUILD-INFO.txt`, and runs `lunepack publish
   fetch`, `pack` (an rc with `--channel beta`) and `setup` for the three targets; **`push`
   (environment `release-push`) waits for the owner's approval**, then runs `lunepack publish push`
   (a dry run first); `attach` (no environment, `GITHUB_TOKEN` only) checks the setup files against the
   digests `pack` reported, attaches them and deletes the `*-build.*` inputs. Every job that holds a
   secret checks the CLI against the `tools-linux` hash before its first `lunepack` call and builds
   nothing.
   After `attach`, the macOS installer: `tools-macos` (GitHub-hosted `macos-14`, `contents: read`, no
   secret; it starts with the run) builds the macOS `lunepack` CLI from the same pin and reports its
   SHA-256, and `dmg` (`macos-14`, `GITHUB_TOKEN` with `contents: write` only, no environment, compiles
   nothing) checks that CLI against it and runs `scripts/package-macos-dmg.sh <tag>`: it builds the
   `luneterm-<version>-macos-arm64-setup.dmg` (+ `.sha256`, unsigned) from the release that is now
   published and attaches both (never replacing an asset; a re-run finds them there and stops). Then
   `drop-artifacts` (`actions: write`, no environment) deletes the run's artifacts, but only when
   `attach` and `dmg` both succeeded. The hosted jobs are the only ones not on the self-hosted
   runner. That GitHub-hosted runners work again (they were blocked by account billing on 2026-11-09)
   is **not verified**: the first run proves it. If `dmg` does not run, run `scripts/package-macos-dmg.sh <tag>`
   on a Mac at the tag (clean tree, `gh` logged in) once `attach` has finished.
4. A stable tag goes to `stable` and `beta`, a release candidate to `beta` only (so no stable user can
   see an rc). Each channel keeps its newest 10 versions (by version precedence); `push` trims the rest
   and deletes a version's objects only when no channel lists it any more.

The workflow also runs a weekly `refresh` (Mondays 05:23 UTC, environment `release-refresh`, default
branch only): a feed lives 90 days (`feed_days`) and the refresh re-signs those that expire within 30 days
(`--if-expiring-within-days 30 --skip-if-unpublished`, so it does nothing until something is
published). GitHub pauses scheduled workflows of a repository with no activity for 60 days, so check the
Actions tab if the repo is quiet. By hand: Actions > publish-updates > Run workflow.

| Dispatch `mode` | Run from | What it does |
|---|---|---|
| `refresh` | the default branch | re-signs every feed |
| `publish` | a `v*` tag ref | `pack`, `push`, `attach`, `dmg` and `drop-artifacts` for that tag (needs its draft published) |
| `drop` + `drop_version` | a `v*` tag ref | takes one release out of the feeds and deletes its objects (`publish drop`; the owner approves) |

`dry_run` (on by default for a dispatch) runs only the `--dry-run` commands (`publish push --dry-run`,
`drop --dry-run`, `refresh --dry-run`) and attaches nothing. A `release: published` run is an apply run.

## One-time setup

The owner's work, done once, on the owner's machine. An agent does not do it and does not touch the keys
it makes (PUBLISHING.md, "Rules for agents"). **Nothing below has been run against real R2 or a real
GitHub environment**: the workflow and `release-pack.py` are tested against a fake `lunepack`, and
`pack`, `setup` and the macOS assemble-and-pack path against the pinned CLI with throwaway keys and a
local feed; `publish push`, the stores and the environments are untested until the first rc.

### Order

Publishing stays off until the last step (the repository variable `PUBLISH_READY`; while it is not
`true`, a release or the weekly schedule ends green with a notice and nothing else runs):

1. the buckets and the domain (done 2026-11-26, below);
2. the key ceremony, the root into `packaging/lunepack.toml` and `publish init` (below);
3. the **three environments with their rules and reviewers, before any secret** (GitHub creates a
   missing environment with no rules when a job names it, and then a branch could reach what you add):
   `release-pack` (tags `v*` only), `release-push` (tags `v*` only; required reviewers: the owner) and
   `release-refresh` (the default branch only), under Settings > Environments > Deployment branches and
   tags; also a tag ruleset limiting who can create `v*` tags, a protected default branch, and no
   repository-level secret of the names below;
4. the secrets of each environment (names below);
5. the runner probe (below);
6. set the repository variable `PUBLISH_READY` to `true` (`gh variable set PUBLISH_READY --body true`).

### Secrets per environment

| Environment | Secrets |
|---|---|
| `release-pack` | `LUNEPACK_RELEASE_KEY` |
| `release-push` | `LUNEPACK_RELEASE_KEY`, `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` (the **push** token; the workflow gives it to both pairs) |
| `release-refresh` | `LUNEPACK_RELEASE_KEY`, `R2_ACCOUNT_ID`, `R2_REFRESH_PUBLIC_ACCESS_KEY_ID`, `R2_REFRESH_PUBLIC_SECRET_ACCESS_KEY` (the **refresh-public** token), `R2_REFRESH_PRIVATE_ACCESS_KEY_ID`, `R2_REFRESH_PRIVATE_SECRET_ACCESS_KEY` (the **refresh-private** token); the workflow maps them onto `[publish]`'s variables |

The secret and variable names are the same as Whipspot's (one standard for lunepack apps).

Set them from files or prompts, never a value on the command line:
`gh secret set LUNEPACK_RELEASE_KEY --env release-push --repo Luneterm/luneterm < keys/release.key`.

The three R2 tokens (Cloudflare dashboard > R2 > Manage R2 API tokens; each shows an **Access Key ID**
and a **Secret Access Key**, the S3 credentials; the "token value" is not used), as in PUBLISHING.md
step 3: **push** (Object Read & Write on both buckets; it also deletes), **refresh-public** (Object Read
& Write on `izzat-updates` only) and **refresh-private** (Object Read only on `izzat-updates-publisher`
only). Never give the refresh job one token that can write both buckets. R2 answers a missing key with
404, so the S3-only `ListBucket` rule does not apply.

**Delete the old repository secrets** (the earlier publisher read them from the repository, which no
job does now): `LUNEPACK_RELEASE_KEY`, `R2_UPDATES_TOKEN`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and
`CLOUDFLARE_ACCOUNT_ID` (now `R2_ACCOUNT_ID`, an environment secret) at repository level (`gh secret list --repo Luneterm/luneterm`, then `gh secret
delete <name>`), once the environments hold the new ones.

### Bucket and domain (done 2026-11-26)

Buckets `izzat-updates` (public) and `izzat-updates-publisher` (private) exist, no CORS, and
`updates.izzat.dev` is attached to `izzat-updates` in the Cloudflare dashboard (R2 > bucket > Settings >
Custom Domains; the zone `izzat.dev` is on the same account, Universal SSL covers the one-level name).
A missing key answers 404. Public access and the custom domain are dashboard settings, not something
a token can change.

### The key ceremony (needs the user)

Two kinds of key: the **root key** signs `keys.json` (the list of release keys) and any later root
rotation; it stays offline. The **release key** signs every feed and manifest; it lives in the three
environments' secret. Do this on a Mac you trust, not on CI. Use a `lunepack` built from the pin
(`scripts/build-lunepack.sh macos-aarch64 <empty dir>` puts it in that directory). The config is the
repository's `packaging/lunepack.toml` (`CFG` below); a `lunepack-state.json` lands beside it
(git-ignored: move it offline with the root key). Work in a directory outside the repository.

```bash
CFG=/path/to/Luneterm/packaging/lunepack.toml
mkdir -p ceremony/keys ceremony/release && cd ceremony && umask 077   # keys set needs release/ to exist
lunepack keys gen keys/root                    # prints public: <hex>; writes keys/root.key
lunepack keys gen keys/release
ROOT=$(lunepack keys show --sign file:keys/root.key | sed -n 's/^public: //p')
REL=$(lunepack keys show --sign file:keys/release.key | sed -n 's/^public: //p')
```

1. **Put the root public key in the config** (one commit): `keys = [...]` of `[root]` in
   `packaging/lunepack.toml` becomes `"$ROOT"`. This replaces the throwaway test root, whose private half
   was discarded: `scripts/release-pack.py check-root` (run by the `pack` job and by
   `package-macos.sh`) refuses any release while the test key is in. A build made before this carries
   the test root: it never accepts an update.
2. **Make `keys.json`** (the release key's window must START BEFORE the commit of the first tag it
   publishes: `pack` signs as of the tag commit's time; macOS `date -v`, GNU `date -d '-1 day'`):

   ```bash
   lunepack --config "$CFG" keys set --release-dir release --release-key "$REL" \
       --not-before "$(date -v-1d +%s)" --not-after "$(date -v+2y +%s)" \
       --expires "$(date -v+2y +%s)" --sign file:keys/root.key
   ```

   A root of several holders (`--root-key` repeated, `--threshold N`) is possible; `threshold` in
   `[root]` must then say the same. `docs/KEY-CEREMONY.md` of lunepack covers thresholds, rotating the
   root and replacing a release key (two moves, the old key stays listed until a feed signed by the new one exists).
3. **Keep offline, with the root key:** `keys/root.key`, `packaging/lunepack-state.json` and
   `release/keys.json` (back them up; `keys set` builds on the state file).
4. **Initialise the store** with the push token in your own shell (the variable names are
   `[publish]`'s; never in CI), dry run first (PUBLISHING.md step 5):

   ```bash
   export R2_ACCOUNT_ID=... R2_ACCESS_KEY_ID=... R2_SECRET_ACCESS_KEY=...   # the push token
   export R2_PRIVATE_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" R2_PRIVATE_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
   lunepack --config "$CFG" publish init --release-dir release --dry-run
   lunepack --config "$CFG" publish init --release-dir release
   ```

5. **Put the release key into the environments** from the file (above), then forget it exists on disk
   outside the offline backup.

Never put a private key, a token or `LUNEPACK_RELEASE_KEY` into a file of the repo, a log line or an
issue. What keeps the release key safe is the job split and the environments, not hiding a variable:
processes of one user in a job can read each other's environment, so a secret must never share a job
with code that is not ours. The key jobs run only the CLI that `tools-linux` built from the pin (its
dependency graph checked first, hash-checked in every key job) and this repository's Python standard
library; `release-pack.py` still takes secrets out of its own environment, gives the release key only to
the `lunepack` children that sign and the token only to the GitHub API. Do not add an npm tool, a
downloaded binary or a third-party action to a key job; give it its own job and no secret of these.

### The runner probe (before the first real publish)

The jobs run on the self-hosted Linux runner (`[self-hosted, linux, x64]`), except `tools-macos` and `dmg`
(GitHub-hosted `macos-14`; neither holds a secret). Whether each job gets a fresh
container, so that a process or file one job leaves cannot be read by a later job holding a secret, has
**not been checked**. Before setting `PUBLISH_READY`, run a throwaway two-job workflow on a pushed
branch: job 1 writes a marker file in `$HOME`, in `/tmp` and in the workspace and leaves a background
process running; job 2 looks for all four. Both jobs also run, with no secret,
`curl -sS -o /dev/null -w '%{http_code}\n' https://updates.izzat.dev/luneterm/keys.json` (expect 200;
`publish fetch` reads the feed the same way, and a CDN rule that answers 403 to the runner's address would
fail `pack`). If a marker or the process survives, a later key job could read an earlier job's
environment: do not publish from that runner until it isolates jobs.

### First release with updates

Rehearse with a release candidate (`v<version>-rc.1`), after the ceremony: it is listed in `beta` only,
so no stable user can see it. Publishing the draft starts the real run (a release event applies); the
`push` job waits for approval. `workflow_dispatch` with `dry_run` on needs a release that is already
published. Then check:

```bash
curl -sI https://updates.izzat.dev/luneterm/feed-beta.json | head -n 5     # 200, no-cache
curl -s -o /dev/null -w '%{http_code}\n' https://updates.izzat.dev/luneterm/feed-stable.json   # 404 until a stable release
lunepack --config packaging/lunepack.toml publish fetch --release-dir /tmp/published-check --version 999999.0.0   # no credential
```

Install the rc setup file on each OS, publish a later rc, and look at About > Check for updates.
Only then tag the stable release.

### Beta releases (from `main` or `develop`)

The beta channel is the pre-release track: a tag `v<version>-rc.<N>` on any commit (on `main`, or on
`develop` for work not ready for `main`) goes through the same release and publish flow and is listed in
the beta feed only. Copies installed from a beta setup update to it; stable copies never see it. The
workspace version in `Cargo.toml` must be `<version>` (the version the rc leads to): bump it on that
branch first. CI runs on pushes to `develop` too; wait for it to be green before tagging. A later stable
`v<version>` is listed in both feeds, so beta copies move to it.

## When something goes wrong

PUBLISHING.md's "Rerun and recovery" and "When something is refused" are the reference. Luneterm's
specifics:

- **A job failed.** Say which case it is:
  - `push` failed part-way (a network error, a timeout) or `attach` failed: re-run only the failed job
    ("Re-run failed jobs" of the same run), never with another version. Once `publish push` has written a
    feed, `publish fetch` refuses that version ("is already published"), so a new run cannot rebuild the
    setup files; the run's artifacts (`lunepack-cli`, `release` and `setup`; `lunepack-cli-macos` too) are
    kept 14 days for exactly this, so the re-run finishes the job within 14 days (the `push` rerun also
    before the next scheduled refresh runs, which re-signs feeds: PUBLISHING.md "Rerun and recovery"). A
    `push` can wait up to 14 days for the owner's approval for the same reason. After that window,
    publish the next version. A successful run deletes its artifacts (`drop-artifacts`); a run where
    `attach` or `dmg` failed keeps them until they expire.
  - `dmg` or `tools-macos` failed, or never started (hosted runners unavailable): the release is already
    published and the update feed is unaffected; only the `.dmg` is missing. Re-run the failed job (it
    stops at once if the `.dmg` is already attached), or run `scripts/package-macos-dmg.sh <tag>` on a Mac
    at the tag. If only one of the `.dmg` and its `.sha256` is on the release, delete it by hand first.
    Until then the macOS `.zip` is the installer.
  - `push` refused with **"the store changed since fetch"**: the store's feeds no longer equal the copies
    `fetch` put in the `release` artifact (usually the weekly `refresh` re-signed a feed after the pack
    job ran). Nothing was written and no feed lists the version yet, so re-running `push` is refused again
    every time. Start a new full run instead: Actions > publish-updates > Run workflow from the `v*` tag,
    mode `publish`, `dry_run` off (or "Re-run all jobs"). It fetches and packs again; the build inputs are
    still on the release because `attach` only runs after a successful `push`.
- **`refresh` or `drop` is refused because a feed changed since that run read it**: another publish or
  refresh wrote meanwhile. Re-run the job.
- **A bad version is live.** Publish a fixed higher version: every copy that checks takes the newest of
  its channel, including copies already on the bad one. A version that crashes before `confirm_healthy`
  is rolled back on the user's machine by lunepack. To stop offering a version without a replacement:
  Actions > publish-updates > Run workflow from its `v*` tag, mode `drop`, `drop_version`, `dry_run` on
  first, then off (the owner approves in `release-push`).
- **The release key leaked, or its window ended.** Replace it with the two moves of lunepack
  `KEY-CEREMONY.md` section 7 (list both keys in a new `keys.json` signed with the root, sign with the new
  one, then list only the new one; `publish keys` uploads it), and set the new `LUNEPACK_RELEASE_KEY` in the
  three environments. No app update is needed. Before `--not-after` of the current key, do the same on
  purpose. The key set itself expires at the `--expires` of the ceremony: repeat `keys set` (give
  `--expires` again, it is not carried over) before then.
- **The root key leaked.** Rotate the root (`KEY-CEREMONY.md` section 8) and treat it as an incident:
  an attacker who got a rotation accepted first can only be undone by a reinstall.
- **A copy says the update failed.** About shows lunepack's message with any URL cut to scheme, host
  and path. "feed expired" means the weekly refresh did not run (refresh by hand); a signature error
  with a new feed means `keys.json` or the root in the installed copy's update config do not match.

## Licences and supply chain

- The lunepack runtime and update window ship in every install (about 540 crates, among them the
  luneui-rs toolkit by git). `lunepack pack` places `lunepack-NOTICES.txt` (every licence text of
  those binaries; macOS: `Contents/Resources/`) in every pack, and lunepack's own gate fails on a
  package with no text; `release-pack.py pack` fails a pack whose manifest lacks it. So the shipped
  lunepack binaries' licences travel with the install and `licenses/` carries only Luneterm's own graph
  (`python3 scripts/gen-licenses.py --check`).
- `scripts/check-deps.sh` covers Luneterm's own graph: two git pins are allowed, each an exact
  `(repo, rev)` (luneui-rs, lunepack at the revision above). The lunepack build graph (its own
  `Cargo.lock`: crates.io plus the one luneui-rs commit its UI links) is **outside** that script:
  `scripts/build-lunepack.sh` and `.ps1` fail closed before building unless every package is
  crates.io, the lunepack checkout itself, or exactly that luneui-rs git source. The CLI that signs is
  built by that script in `tools-linux`.
- Bumping the lunepack pin: change `Cargo.toml`, `scripts/check-deps.sh` and rerun
  `python3 scripts/gen-licenses.py`; the feed format may change between lunepack revisions, so
  install an older build from a setup file, update it through a `file:` feed to a newer local pack
  and force a rollback (a version that never confirms), on all three OSes, before releasing.
- Network exposure: the app opens HTTPS to `updates.izzat.dev` only (ureq with rustls and the
  platform verifier, through lunepack), only when a check or download was asked for; feeds are
  verified from the installed copy's root before anything is read from them, and the app treats all
  feed content as untrusted input (lunepack parses it).
