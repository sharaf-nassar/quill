---
lat:
  require-code-mention: true
---
# Pi Package Test Specs

These tests pin the public Pi package artifact and its supported host contract.

## Registry artifact

The npm tarball exports one dependency-free extension and contains only its manifest, license, README, and shared production source.

## Desktop-first publication

`.github/workflows/publish-pi-extension.yml` requires the matching published desktop release and managed assets before injecting that exact build into the reporter, running package/provenance dry runs, and publishing the npm artifact.
