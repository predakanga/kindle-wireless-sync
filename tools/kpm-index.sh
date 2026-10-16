#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Writes the KPM repository index (repo.json, ADR 0014) to stdout: one
# artifact per published vX.Y.Z release that has the package attached,
# newest first. Name, author, description and platforms come from
# package/manifest.json.in, so they match the package's own manifest.
#
# Usage: tools/kpm-index.sh RELEASES.json OWNER/REPO
#   RELEASES.json: a JSON array from the GitHub Releases API, e.g.
#   gh api --paginate repos/OWNER/REPO/releases --jq '.[]' | jq -s .
set -eu

releases=$1
repo=$2

sed 's|@VERSION@|0,0,0|' package/manifest.json.in |
jq --slurpfile rel "$releases" --arg repo "$repo" '
  . as $m
  | {
      manifest_version: 1,
      id: $m.id,
      name: $m.name,
      description: "Releases of \($m.name) (https://github.com/\($repo))",
      packages: {
        ($m.id): {
          name: $m.name,
          author: $m.author,
          description: $m.description,
          artifacts: [
            $rel[0][]
            | select((.draft | not) and (.prerelease | not))
            | select(.tag_name | test("^v[0-9]+\\.[0-9]+\\.[0-9]+$"))
            | select(any(.assets[]; .name == "\($m.id).kpkg"))
            | {
                url: "https://github.com/\($repo)/releases/download/\(.tag_name)/\($m.id).kpkg",
                version: (.tag_name[1:] | split(".") | map(tonumber)),
                dependencies: [],
                supported_platforms: $m.supported_platforms
              }
          ] | sort_by(.version) | reverse
        }
      }
    }'
