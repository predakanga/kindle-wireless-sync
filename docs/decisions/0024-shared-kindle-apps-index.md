# 0024: Publish through the shared kindle-apps KPM index

- Status: Proposed
- Date: 2026-10-20
- Amends: [0014](0014-release-and-distribution.md) (the KPM index on this
  repository's GitHub Pages)

## Context
ADR 0014 built a KPM index for this package alone, on this repository's
GitHub Pages. The developer now publishes one index for all their
packages, `predakanga/kindle-apps`, and asked for this repository's
release to use it (2026-10-20). The design is recorded in Kindle Series
Enabler's [ADR 0005](https://github.com/predakanga/kindle-series-enabler/blob/main/docs/decisions/0005-one-index-for-all-packages.md),
which already planned this move.

In short: kindle-apps' `packages.txt` lists the app repositories (this one
is already there), and its *Publish the index* workflow builds
`https://predakanga.github.io/kindle-apps/repo.json` (index id
`kindle-apps`) from every published `vX.Y.Z` release with a `.kpkg`. The
package's details come from the `manifest.json` inside each `.kpkg`. That
index already lists every release of this package, 0.1.0 to 0.4.0.

Unlike Kindle Series Enabler, this package has users of the old index
(at least the reference device). From KPM 0.2.2's source (`cli/main.c`,
`src/package.c`, `src/install.c`, at `07495e0`):
- `kpm upgrade` only looks in the repository a package was installed
  from, so it would never see a release published only in kindle-apps.
- `kpm install <id>` without a repository takes the newest artifact from
  any repository (`ORDER BY` version only), and installs an equal or newer
  version through the upgrade path. It then records the package against
  the repository it came from (`INSERT OR REPLACE INTO
  installed_packages`).

## Decision
**Release workflow:** the `pages` job and the `workflow_dispatch` trigger
go. After publishing the GitHub Release, the job starts kindle-apps'
`publish.yml` with `gh workflow run`, using the `KINDLE_APPS_TOKEN`
secret: a fine-grained token for kindle-apps only, with **Actions** read
and write. The step is copied from Kindle Series Enabler's
`.github/workflows/release.yml`. Without the secret, it warns and the
release still succeeds; the index can be rebuilt by hand from kindle-apps'
Actions tab.

`tools/kpm-index.sh` and `package/pages/index.html` are removed. Their
successor is kindle-apps' `tools/build-index.sh` and `pages/index.html`.

**Install instructions** (README) use the kindle-apps URL. Users of the
old repository move over by removing it, adding kindle-apps, and running
`kpm install kindle-wireless-sync`. Removing the old repository first
means the install can only take kindle-apps' artifact, and the package
is then recorded against kindle-apps, so `kpm upgrade` works from then on.

## Alternatives considered
- **Keep this repository's index as well** (dispatch to kindle-apps and
  rebuild our Pages): old users would keep getting upgrades without doing
  anything, but there are two indexes to keep working, and two
  repositories on a device listing the same package make `kpm install`
  pick between them by version only. There are few users to move.
- **Keep only our own index:** what the developer asked to replace.

## Consequences
- **Setup by the developer:** add the `KINDLE_APPS_TOKEN` secret to this
  repository: the same token as Kindle Series Enabler's, or a new one
  made as kindle-apps' README describes.
- This repository's Pages site stays frozen at 0.4.0 until it's turned off.
  Leaving it up for a while lets old users still `kpm update` without an
  error. Once it's off, the `github-pages` environment and the Pages
  setting from ADR 0014 can go too.
- **To verify on the device:** the move from the old repository (remove,
  add, `kpm install`) keeps settings and books and records the package
  against `kindle-apps` (KPM's `installed_packages` table).
- The release workflow can't tell whether the index was rebuilt: the
  dispatch is fire-and-forget, and kindle-apps' own run shows the result.
