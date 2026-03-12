#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./release.sh <command> [args]

Commands:
  bump <major|minor|patch>   Create and push a new version tag
  retag [version]            Replace an existing tag locally and remotely (defaults to latest)
  latest                     Show the latest version tag

Examples:
  ./release.sh bump patch        # v0.2.1 -> v0.2.2
  ./release.sh bump minor        # v0.2.1 -> v0.3.0
  ./release.sh bump major        # v0.2.1 -> v1.0.0
  ./release.sh retag 0.2.1       # Re-point v0.2.1 to current HEAD
  ./release.sh latest            # Print latest tag
EOF
  exit 1
}

get_latest_tag() {
  git tag --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -n1
}

parse_version() {
  local tag="$1"
  echo "${tag#v}"
}

bump_version() {
  local version="$1" part="$2"
  local major minor patch
  IFS='.' read -r major minor patch <<< "$version"

  case "$part" in
    major) echo "$((major + 1)).0.0" ;;
    minor) echo "${major}.$((minor + 1)).0" ;;
    patch) echo "${major}.${minor}.$((patch + 1))" ;;
    *) echo "Invalid part: $part" >&2; exit 1 ;;
  esac
}

cmd_bump() {
  local part="${1:-}"
  if [[ -z "$part" || ! "$part" =~ ^(major|minor|patch)$ ]]; then
    echo "Usage: ./release.sh bump <major|minor|patch>"
    exit 1
  fi

  local latest current new_version
  latest=$(get_latest_tag)
  if [[ -z "$latest" ]]; then
    current="0.0.0"
  else
    current=$(parse_version "$latest")
  fi

  new_version=$(bump_version "$current" "$part")
  echo "Current version: ${current}"
  echo "New version:     ${new_version}"
  echo ""

  read -rp "Create and push tag v${new_version}? [Y/n] " confirm
  if [[ "$confirm" == [nN] ]]; then
    echo "Aborted."
    exit 0
  fi

  git tag "v${new_version}"
  git push origin "v${new_version}"
  echo "Pushed v${new_version} - CI release workflow will start automatically."
}

cmd_retag() {
  local version="${1:-}"
  if [[ -z "$version" ]]; then
    local latest
    latest=$(get_latest_tag)
    if [[ -z "$latest" ]]; then
      echo "No version tags found."
      exit 1
    fi
    version=$(parse_version "$latest")
  fi

  # Strip v prefix if provided
  version="${version#v}"
  local tag="v${version}"

  if ! git tag -l "$tag" | grep -q .; then
    echo "Tag $tag does not exist locally."
    exit 1
  fi

  echo "This will re-point $tag to HEAD ($(git rev-parse --short HEAD))."
  echo "WARNING: This deletes the tag on the remote and re-pushes it."
  echo ""

  read -rp "Continue? [Y/n] " confirm
  if [[ "$confirm" == [nN] ]]; then
    echo "Aborted."
    exit 0
  fi

  git tag -d "$tag"
  git tag "$tag"
  git push origin ":refs/tags/$tag"
  git push origin "$tag"
  echo "Re-tagged $tag to $(git rev-parse --short HEAD) locally and remotely."
}

cmd_latest() {
  local latest
  latest=$(get_latest_tag)
  if [[ -z "$latest" ]]; then
    echo "No version tags found."
  else
    echo "$latest ($(parse_version "$latest"))"
  fi
}

[[ $# -lt 1 ]] && usage

command="$1"
shift

case "$command" in
  bump)   cmd_bump "$@" ;;
  retag)  cmd_retag "$@" ;;
  latest) cmd_latest "$@" ;;
  *)      usage ;;
esac
