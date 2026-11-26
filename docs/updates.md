# Updates

Luneterm installs through [lunepack](https://github.com/lune-rs/lunepack) setup files and updates
itself from a signed feed on Cloudflare R2. This page is for the person who publishes: the one-time
setup (bucket, domain, keys, secrets), what a release does, and what to do when something goes wrong.
How the app side works is in `docs/architecture.md` "Updates". lunepack is pre-release; Luneterm pins
one revision (`Cargo.toml`, `scripts/check-deps.sh`), and the lunepack docs at that revision are the
reference for every `lunepack` command below:
<https://github.com/lune-rs/lunepack/tree/a3d23376a89b3825f9cc92e674af8ce04bfc09e4/docs>
(`CLI.md`, `FORMAT.md`, `KEY-CEREMONY.md`).

## What users get

| | |
|---|---|
| Installers | `luneterm-<version>-linux-x64-setup` (run `chmod +x` first), `luneterm-<version>-windows-x64-setup.exe`, `luneterm-<version>-macos-arm64-setup.zip` (holds `Luneterm.app`: move it to `/Applications` or `~/Applications`; lunepack adopts it on the first update check). Each has a `.sha256` beside it, attached to the GitHub release. All unsigned (no Developer ID, no Authenticode), so each OS warns on first launch. |
| Installed to | macOS `~/Applications/Luneterm.app` (state in `~/Library/Application Support/my.ijat.luneterm`), Linux `~/.local/share/my.ijat.luneterm` (command link `~/.local/bin/luneterm`), Windows `%LOCALAPPDATA%\Programs\Luneterm`. Per user, no admin rights. |
| Updating | Settings > About: "Check for updates automatically" (on by default: at launch and every 24 h, a found version shows a toast with Update / Dismiss), "Check for updates", and "Restart to update" once an update is downloaded and staged. Restarting closes all shells. |
| Not installed by lunepack | A `cargo run` build or an old portable zip says "Updates need the Luneterm installer" and never touches the network. |
| Rollback | An update that does not confirm itself (`confirm_healthy`, after the first frame and the first shell) within lunepack's health timeout is rolled back to the previous version by lunepack's runtime. |
| Channels | `stable` and `beta`. A copy follows the channel its installer came from: the first run records it as `update_channel` in `settings.json` and keeps it, whatever a later pack says (a beta copy that updates to a stable pack stays on beta). To switch, uninstall and run the other channel's setup file (a setup does not run over an install): the first start after a setup records that setup's channel, whatever `settings.json` (which survives the uninstall) held. Limit: a per-machine install gets no such signal from lunepack and keeps the recorded channel; edit `update_channel` (`stable` or `beta`) in `settings.json` to change it. |

## Where everything lives

| Piece | Where |
|---|---|
| Feed (public) | R2 bucket `izzat-updates`, prefix `luneterm/`, served at `https://updates.izzat.dev/luneterm/` (one shared update host for the user's apps; the Whipspot app uses `whipspot/`). The URL is compiled into every installed copy: `[update] feed` in `packaging/lunepack-<os>.toml`. |
| Publisher state (private) | R2 bucket `izzat-updates-publisher`, key `luneterm/lunepack-state.json`: lunepack's record of what was last signed, so an older copy of a feed cannot be written back. Never public. |
| Feed layout | `luneterm/keys.json`, `luneterm/root-<k>.json` (after a root rotation), `luneterm/feed-stable.json`, `luneterm/feed-beta.json`, and per version `luneterm/<version>/<target>.{pack,manifest.json,from-<old>.delta}`, targets `linux-x86_64`, `windows-x86_64`, `macos-aarch64`. |
| Root public key | Compiled into the app (`crates/luneterm/src/update/root.rs`) and written in the `[root]` table of the three `packaging/lunepack-{linux,windows,macos}.toml` (one file per OS because `[install] entry` differs; a unit test in `root.rs` keeps them equal). The private root key stays offline with the user. |
| Release signing key | GitHub secret `LUNEPACK_RELEASE_KEY` of the repo; used only by `publish-updates.yml`. |
| Publisher | `scripts/publish-updates.py` (standard library only; tests: `python3 -m unittest scripts/test_publish_updates.py`) run by `.github/workflows/publish-updates.yml`. |
| Build inputs | `.github/workflows/release.yml` (Linux, Windows) and `scripts/package-macos.sh` (macOS) build the app and lunepack's runtime for each OS (`scripts/build-lunepack.sh`, `scripts/build-lunepack.ps1`; format: `scripts/make-build-input.py`). |

## How a release reaches users

1. Push the tag `v<version>` (or `v<version>-rc.<N>`; the version must equal the workspace version).
   `release.yml` verifies it, builds the Linux and Windows build inputs and creates a draft release
   holding them (`luneterm-<version>-linux-x64-build.tar.gz`, `...-windows-x64-build.zip`, each with
   a `.sha256`). These are not user downloads.
2. On a Mac at the tagged commit: `scripts/package-macos.sh <tag>` builds, assembles and ad-hoc signs
   the app for lunepack and attaches `luneterm-<version>-macos-arm64-build.tar.gz` (+ `.sha256`) to the
   draft. There is no macOS CI.
3. Publish the draft: `gh release edit <tag> --draft=false`. That starts `publish-updates.yml`
   (single-use Linux container; the only job that holds the release key and the R2 access key, and it
   runs no third-party code: the pinned lunepack CLI and the Python standard library). It packs
   each target with `lunepack pack --deltas 3` (deltas against the newest 3 earlier versions of the
   channel), adds the version to the feeds (a stable tag to `stable` and `beta`, a release candidate
   to `beta` only; 365-day expiry), makes the setup files, writes to R2, prunes, then attaches the
   setup files to the GitHub release and deletes the `*-build.*` inputs from it.
4. Write order, so a failure never leaves clients with a feed pointing at missing files: new
   `<version>/` objects (packs and deltas before manifests), then (after checking that no live feed
   changed since it was read) the record of the deletes still to do, then the feeds, then the state
   file, then the deletes. A failed upload stops the run before any feed is written. Rerunning a
   publish of the same tag is idempotent (it reuses what the bucket already holds).

The publisher refuses to run while the app or any packaging config carries the throwaway **test root**
(see D9 below), and without `--apply` it only prints what it would do (`workflow_dispatch` defaults to a
dry run; a release or the weekly schedule applies).

### Retention

Each channel lists only its newest 10 versions (by version precedence: `0.9.0-rc.2` is older than
`0.9.0`). Older ones are dropped from that channel's feed (`feed refresh --drop`). A `<version>/`
prefix is deleted from R2 only when **no** channel lists it any more: a stable version that falls out
of stable but is still in beta stays. The objects to delete are named from that version's own
manifests (pack and deltas), never by listing the bucket, and every key deleted starts with
`luneterm/` and has the shape `luneterm/<version>/<file>`: `keys.json`, `root-*.json`, the feeds and
the state file are never deletable by the script. A version that was the base of a newer version's
delta may be deleted: a client takes a delta only from the version it runs, otherwise the full pack.
The keys to delete are recorded in the private bucket (`izzat-updates-publisher`,
`luneterm/pending-deletes.json`) before any feed is written, and a delete that fails does not stop
the others; the run then fails and names the keys left. The next run, a publish or a refresh (the
weekly one too), loads that record, adds its own, deletes them all again (deleting a key that is
gone is not an error) and empties it, so a failed delete costs a rerun, not an orphaned `<version>/`.
A recorded key whose version has been listed again in the meantime is dropped from the record, not
deleted. A record that does not parse stops the run before anything is written; fix or delete that
one file by hand.
Preview it for a local release directory: `python3 scripts/publish-updates.py plan --release-dir <dir>
[--add <version>] [--keep N]`.

### Feed expiry and refresh

A feed expires 365 days after it was signed; a client with an expired feed sees an error, not an
update. `publish-updates.yml` re-signs every feed that expires within 120 days once a week (schedule,
Mondays 05:23 UTC; GitHub pauses scheduled workflows of a repository with no activity for 60 days, so
check the Actions tab if the repo is quiet) and on demand: Actions > publish-updates > Run workflow,
mode `refresh`, `dry_run` off. A refresh of a feed that is not close to expiry is harmless (a new
`seq`, a new expiry).

## One-time setup

### Bucket and domain (done 2026-11-26)

Buckets `izzat-updates` (public) and `izzat-updates-publisher` (private) exist, no CORS, and
`updates.izzat.dev` is attached to `izzat-updates` in the Cloudflare dashboard (R2 > bucket > Settings >
Custom Domains; the zone `izzat.dev` is on the same account, Universal SSL covers the one-level name).
A missing key answers 404. Public access and the custom domain are dashboard settings, not something
a token can change. Feeds are uploaded with `Cache-Control: no-cache`, version objects with
`public, max-age=31536000, immutable`.

### The R2 access key

Create an R2 API token for the publisher: Cloudflare dashboard > R2 > Manage R2 API tokens > Create,
permission Object Read & Write, scoped to the two buckets above only (not account-wide). Creating it
shows an **Access Key ID** and a **Secret Access Key** (the S3 credentials; the "token value" is not
used). The publisher talks to R2's S3 API (`https://<account id>.r2.cloudflarestorage.com`, path
style, region `auto`) and signs each request itself with AWS Signature Version 4, written on the Python
standard library (`sigv4_authorization` in `scripts/publish-updates.py`, tested against the worked
examples of AWS's documentation). No npm package, `wrangler` or other third-party program runs in the
publish job. The signer is known to be right for AWS's documented examples; the scoped key against
R2 is untested until its first use (a dry run reads `keys.json` and the state file, so it is a cheap
first check), so if R2 answers `AccessDenied` or `SignatureDoesNotMatch`, the error names the S3 code
(never a header): check the scope and the account id before anything else.

### The key ceremony (D9: needs the user)

Two kinds of key: the **root key** signs `keys.json` (the list of release keys) and any later root
rotation; it stays offline. The **release key** signs every feed and manifest; it lives in the GitHub
secret. Do this on a Mac you trust, not on CI. Use a `lunepack` built from the pin
(`scripts/build-lunepack.sh macos-aarch64 <empty dir>` puts it in that directory), so the keys and
documents are the version the app reads.

```bash
mkdir -p ceremony/keys ceremony/release && cd ceremony && umask 077
lunepack keys gen keys/root                    # prints public: <hex>; writes keys/root.key
lunepack keys gen keys/release
ROOT=$(lunepack keys show --sign file:keys/root.key | sed -n 's/^public: //p')
REL=$(lunepack keys show --sign file:keys/release.key | sed -n 's/^public: //p')
lunepack init --app-id my.ijat.luneterm --root-key "$ROOT"   # lunepack.toml + lunepack-state.json
# keys.json: the release key's window must START BEFORE the commit of the first tag it publishes
# (pack signs as of the tag commit's time). macOS date: -v; GNU date: -d '-1 day'.
lunepack keys set --release-dir release --release-key "$REL" \
    --not-before "$(date -v-1d +%s)" --not-after "$(date -v+2y +%s)" \
    --expires "$(date -v+2y +%s)" --sign file:keys/root.key
```

`keys.json` now sits in `ceremony/release/`. A root of several holders (`--root-key` repeated,
`--threshold N`) is possible; `ROOT_THRESHOLD` in `root.rs` and `threshold` in the three configs must
then say the same. See lunepack `docs/KEY-CEREMONY.md` for thresholds, rotating the root and replacing
a release key (two moves, the old key stays listed until a feed signed by the new one exists).

1. **Keep offline, with the root key:** `keys/root.key`, `lunepack.toml`, `lunepack-state.json` and
   `release/keys.json` (back them up; `keys set` needs the state file to build on).
2. **Upload `keys.json`** to `izzat-updates/luneterm/keys.json` (content type `application/json`,
   cache control `no-cache`): in the Cloudflare dashboard (R2 > `izzat-updates` > Upload, into the
   `luneterm/` folder, then set the two headers under the object's settings), or with any S3 client
   pointed at `https://<account id>.r2.cloudflarestorage.com` with your own credentials, for example
   `aws s3api put-object --endpoint-url ... --bucket izzat-updates --key luneterm/keys.json --body
   release/keys.json --content-type application/json --cache-control no-cache`. Do this on your own
   machine, never in the publish job.
3. **Put the release key in the repo's secret store** from the file, never from the command line:
   `gh secret set LUNEPACK_RELEASE_KEY --repo Luneterm/luneterm < keys/release.key`.
4. **Other secrets** of the same repo: `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY` (the key pair
   above: `gh secret set R2_ACCESS_KEY_ID --repo Luneterm/luneterm`, likewise the secret, paste at the
   prompt) and `CLOUDFLARE_ACCOUNT_ID` (the 32-hex account id; `gh secret set CLOUDFLARE_ACCOUNT_ID
   --repo Luneterm/luneterm`). The older `R2_UPDATES_TOKEN` is no longer read: delete it. No other
   workflow reads these; `release.yml` and `ci.yml` never see the release key.
5. **Compile the root public key in** (one commit): the `keys = [...]` list of the `[root]` table in
   `packaging/lunepack-linux.toml`, `-windows.toml` and `-macos.toml`; `ROOT_KEYS` in
   `crates/luneterm/src/update/root.rs` as 32 bytes (`python3 -c 'import sys; h=sys.argv[1];
   print(", ".join("0x"+h[i:i+2] for i in range(0,64,2)))' "$ROOT"`); `IS_TEST_ROOT = false`, and
   delete the `TEST_ROOT_KEY` constant and its mentions. Then `cargo test -p luneterm --lib update`
   (the configs must equal the compiled-in root) and the Python tests: `TestRoot.
   test_the_repo_today_refuses_to_publish` records that today's repo is the test root and must be
   changed in the same commit to expect a pass.
6. Only now tag a release. A build made before this step carries the test root, whose private half
   was discarded: it never accepts an update, and the publisher refuses it.

Never put a private key, a token or `LUNEPACK_RELEASE_KEY` into a file of the repo, a log line or an
issue. How the publisher treats them: it takes the secrets out of its environment at start, hands the
release key only to the `lunepack` children, signs the R2 requests itself and sends `GITHUB_TOKEN`
only to the GitHub API, and redacts what it echoes. That is hygiene, not a boundary: processes of one
user in a job can read each other's environment (`/proc/<pid>/environ`, which clearing a variable in
a parent does not erase). What keeps the release key safe is that the job runs **no third-party
code**: the lunepack CLI built from the pin (its dependency graph checked first) and the Python
standard library. Do not add an npm tool, a downloaded binary or a third-party action to
`publish-updates.yml` without giving it its own job (its own runner) and no secret of this one.

### First release with updates

The first publish has no state file and no feed: `publish-updates.py` takes the bucket's `keys.json`
as the baseline (`--trust-release-dir`, once) and writes the state file with the first feeds. It
fails closed when a feed is live but the state file is missing.

Rehearse with a release candidate (`v<version>-rc.1`): it is listed in `beta` only, so no stable user
can see it. Publishing the draft starts the real run (the workflow applies on `release: published`);
`workflow_dispatch` with `dry_run` on needs a release that is already published. Then check:

```bash
curl -sI https://updates.izzat.dev/luneterm/feed-beta.json | head -n 5     # 200, no-cache
curl -s -o /dev/null -w '%{http_code}\n' https://updates.izzat.dev/luneterm/feed-stable.json   # 404 until a stable release
```

Install the rc setup file on each OS, publish a later rc, and look at About > Check for updates.
Only then tag the stable release.

## When something goes wrong

- **Publish failed before the feeds were written** (upload, GitHub, runner): rerun the same tag from
  Actions > publish-updates (mode `publish`, the tag, `dry_run` off). It reuses what is already in the
  bucket. If the draft's `*-build.*` inputs are gone already, the objects were published: only the
  installers or the deletes may be missing. A rerun attaches the installers and does every delete
  still recorded in `izzat-updates-publisher/luneterm/pending-deletes.json` (the weekly or a manual
  refresh does the deletes too). If a run ended with "delete(s) failed", its message names the keys
  left; they stay recorded, so the next run retries them. Nothing else lists the bucket for strays:
  an object that was never recorded (a run that died before it recorded the deletes) is found only by
  looking at the bucket by hand.
- **A feed changed meanwhile** ("writing no feed"): another publish ran. The runs are serialised by
  the workflow's concurrency group; rerun.
- **A missing state file while a feed is live** ("not starting over"): restore
  `luneterm/lunepack-state.json` of `izzat-updates-publisher` from a backup. Do not use
  `--trust-release-dir` to get past it: that disables the replay check.
- **A bad version is live.** Publish a fixed higher version: every copy that checks takes the newest
  of its channel, including copies already on the bad one. A version that crashes before
  `confirm_healthy` is rolled back on the user's machine by lunepack. To stop offering a version
  without a replacement, drop it from the feeds by hand (not automated; `feed refresh --drop` then an
  update was tried on Linux and Windows with a `file:` feed): pull the state file, `keys.json`, the feeds and the packs of the listed
  versions into a release directory, run `lunepack feed refresh --release-dir <dir> --channel
  <stable|beta> --drop <version> --sign file:<release key file> --expires-after-days 365 --config
  packaging/lunepack-linux.toml` for each channel that lists it, upload the feeds with
  `Cache-Control: no-cache` and then the state file, and delete `luneterm/<version>/` by hand. The
  script's own order and checks do not run, so go slowly and check the result with `lunepack verify
  <dir>` before uploading.
- **The release key leaked, or its window ended.** Replace it with the two moves of lunepack
  `KEY-CEREMONY.md` section 7 (list both keys in a new `keys.json` signed with the root, sign with
  the new one, then list only the new one), upload `keys.json`, and set the new `LUNEPACK_RELEASE_KEY`.
  No app update is needed. Before `--not-after` of the current key, do the same on purpose. The
  key set itself expires at the `--expires` of the ceremony: repeat `keys set` (give `--expires`
  again, it is not carried over) before then and upload `keys.json`.
- **The root key leaked.** Rotate the root (`KEY-CEREMONY.md` section 8; upload `root-<k>.json`
  before `keys.json`) and treat it as an incident: an attacker who got a rotation accepted first
  can only be undone by a reinstall.
- **A copy says the update failed.** About shows lunepack's message with any URL cut to scheme, host
  and path. "feed expired" means the weekly refresh did not run (refresh by hand); a signature error
  with a new feed means `keys.json` or the root do not match the compiled-in key.

## Licences and supply chain

- The lunepack runtime and update window ship in every install (about 540 crates, among them the
  luneui-rs toolkit by git). `lunepack pack` places `lunepack-NOTICES.txt` (every licence text of
  those binaries; macOS: `Contents/Resources/`) in every pack, and lunepack's own gate fails on a
  package with no text; the publisher fails a pack whose manifest lacks it. So the shipped lunepack
  binaries' licences travel with the install and `licenses/` carries only Luneterm's own graph
  (`python3 scripts/gen-licenses.py --check`).
- `scripts/check-deps.sh` covers Luneterm's own graph: two git pins are allowed, each an exact
  `(repo, rev)` (luneui-rs, lunepack at the revision above). The lunepack build graph (its own
  `Cargo.lock`: crates.io plus the one luneui-rs commit its UI links) is **outside** that script:
  `scripts/build-lunepack.sh` and `.ps1` fail closed before building unless every package is
  crates.io, the lunepack checkout itself, or exactly that luneui-rs git source.
- Bumping the lunepack pin: change `Cargo.toml`, `scripts/check-deps.sh` and rerun
  `python3 scripts/gen-licenses.py`; the feed format may change between lunepack revisions, so
  install an older build from a setup file, update it through a `file:` feed to a newer local pack
  and force a rollback (a version that never confirms), on all three OSes, before releasing.
- Network exposure: the app opens HTTPS to `updates.izzat.dev` only (ureq with rustls and the
  platform verifier, through lunepack), only when a check or download was asked for; feeds are
  verified from the compiled-in root before anything is read from them, and the app treats all
  feed content as untrusted input (lunepack parses it).
