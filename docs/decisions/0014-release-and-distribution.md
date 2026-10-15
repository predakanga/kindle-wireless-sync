# 0014: Releases on GitHub, KPM index on GitHub Pages

- Status: Accepted
- Date: 2026-10-15

## Context
KPM 0.2.2 can't install a local `.kpkg` over an installed package (ADR 0012,
findings), but it upgrades correctly from a repository. So users need a KPM
repository (a `repo.json` index) to get upgrades without losing their
settings. The developer chose to host it on **GitHub Pages**, updated by
GitHub Actions on each release.

Evidence:
- **GitHub hosting works on the reference device.** Kindle HID Passthrough's
  index (`https://lzampier.com/hid`) redirects to `raw.githubusercontent.com`.
  Its artifact is a versioned GitHub Release asset,
  `…/releases/download/v3.17.5/kindle-hid-passthrough.kpkg`. The device's KPM
  installed it from there. KPM sets `CURLOPT_FOLLOWLOCATION`
  (`src/install.c`), so it follows the redirects.
- **Index format** (`src/index.c`): `manifest_version` 1, `id`, `name`,
  `description`, and `packages.<id>` with `name`, `author`, `description`
  and `artifacts[]` (`url`, `version`, `dependencies`,
  `supported_platforms`). An artifact whose platforms don't include the
  device's is skipped at index time.
- **Several versions:** KPM lists a package's artifacts newest first
  (`src/package.c`, `ORDER BY version_major DESC, …`) and installs the first,
  so an index can list every release. The KMC repo does this for KOReader.
- **`kpm upgrade` skips packages installed from a local file** ("installed
  locally - skipping"). `kpm install <id>` from a repository still upgrades
  them (tested in ADR 0012).
- **Prior art:** Kindle Button Mapper's `.github/workflows/build-arm.yml`
  builds the armv7-musl binary, checks that `manifest.json` is at the root
  of the `.kpkg`, and attaches it to a GitHub Release with
  `softprops/action-gh-release`. Its `kpm/gen-manifests.sh` generates the
  manifest and `repo.json` from `Cargo.toml`, and CI fails if the committed
  `repo.json` is stale.

## Decision
**Release trigger:** pushing a tag `vX.Y.Z`. The workflow fails unless the
tag matches the `version` in `Cargo.toml`.

**Release workflow** (`.github/workflows/release.yml`, `ubuntu-latest`):
1. Run `just check` and `cargo test`.
2. Build the package with the same recipe used locally (`just package`).
   The tar step moves into a script that works with both GNU tar (CI) and
   bsdtar (macOS): `manifest.json` at the root, root-owned entries, no
   macOS metadata.
3. Check the archive: `manifest.json` at the root, the binary is ARM, and
   `discover` is absent (no `dev-tools`).
4. Create the GitHub Release for the tag, with `kindle-wireless-sync.kpkg`
   attached.
5. Rebuild and deploy the Pages site (below).

**Pages site** (deployed with `actions/upload-pages-artifact` and
`actions/deploy-pages`; nothing generated is committed):
- `repo.json`: the KPM index, with `id` `kindle-wireless-sync`. It lists
  **every published release** as an artifact, built from the Releases API.
  Each URL is the versioned release asset
  (`…/releases/download/vX.Y.Z/kindle-wireless-sync.kpkg`).
  `supported_platforms` is `["kindlehf"]` (ADR 0001). The owner and repo
  name come from `github.repository`, so nothing is hard-coded.
- `index.html`: a short page with the `kpm add-repo` URL and install steps.

**Install instructions** tell users to add the repository and install from
it (`kpm add-repo <pages-url>/repo.json`, `kpm update`,
`kpm install kindle-wireless-sync`), not from a downloaded file, so that
upgrades work.

## Alternatives considered
- **A committed `repo.json`, served from `raw.githubusercontent.com`**
  (Button Mapper, HID Passthrough): proven, but every release needs a commit
  that bumps the index, plus a staleness check. With Pages built at release
  time, the index can't go stale.
- **Hosting the `.kpkg` on Pages** (relative artifact URLs, as the KMC repo
  does): this would make the Pages site the archive of every release. Release
  assets already are that, and they are proven on the device.
- **Listing only the latest release:** simpler. Listing all of them costs
  nothing and keeps older versions installable by version.
- **A `latest/download` URL** (Button Mapper's `repo.json`): rejected. The
  index's version must match the file it points to.

## Consequences
- **One-time setup by the developer:** the GitHub repository, then
  *Settings → Pages → Source: GitHub Actions*. The workflow needs
  `contents: write`, `pages: write` and `id-token: write`.
- `just package` must produce the same archive on macOS and in CI. The tar
  step becomes a script with a branch for each tar flavour.
- **To verify on the device:** adding the Pages URL with `kpm add-repo` and
  installing from it. KPM issue #5 reports that `add-repo` with a URL
  "silently returns to the home screen", which may affect the on-device
  launcher rather than the CLI. Check how users run `kpm add-repo` before
  writing the instructions.
- A user who first installed from a file can move to the repository by
  adding it and running `kpm install kindle-wireless-sync`. Their settings
  are kept, because that runs the upgrade path (ADR 0012).

## Implementation notes (2026-10-15)
- The repository is `predakanga/kindle-wireless-sync`. The index URL is
  `https://predakanga.github.io/kindle-wireless-sync/repo.json`, and the
  licence is GPLv3 (`LICENSE`; `Cargo.toml` says `GPL-3.0-or-later`).
- `.github/workflows/release.yml` creates the release with the runner's `gh`
  CLI rather than `softprops/action-gh-release`, which leaves one
  third-party action fewer.
- `tools/package.sh` builds the `.kpkg` for both `just package` and CI, and
  handles bsdtar and GNU tar. `tools/kpm-index.sh` writes `repo.json` from
  the Releases API, skipping drafts, pre-releases and releases without the
  package. `package/pages/index.html` is the install page.
- **Setup:** besides *Pages → Source: GitHub Actions*, the `github-pages`
  environment must allow deployments from `v*` tags. Its default branch
  policy only allows the default branch, and the deploy runs on the tag.
- **How to add the repository:** HID Passthrough's README tells users to
  run `kpm add-repo <url>` in a shell (kterm or SSH), and that's how it was
  added on the reference device. Our page does the same. The search-bar form
  (`;kpm add-repo <url>`) is the one that KPM issue #5 reports as silently
  returning to the home screen.
