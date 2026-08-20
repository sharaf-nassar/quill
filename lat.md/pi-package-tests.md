---
lat:
  require-code-mention: true
---
# Pi Package Test Specs

These tests pin the public Pi package artifact and its supported host contract.

## Registry artifact

The npm tarball exports one dependency-free extension and contains only its manifest, license, README, and shared production source.

## Desktop-first publication

Pi publication requires one verified matching desktop release before npm publication.

`.github/workflows/publish-pi-extension.yml` reads draft, prerelease, and asset metadata in one release lookup, requires managed assets, injects that exact build into the reporter, runs package/provenance dry runs, then publishes.
