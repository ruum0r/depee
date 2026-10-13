#!/bin/sh
set -eu

# Everything runs from main so a truncated download (e.g. `curl | sh` cut off
# mid-transfer) executes nothing.
main() {
  repo="ruum0r/depee"
  install_dir="${DEPEE_INSTALL_DIR:-$HOME/.local/bin}"
  # DEPEE_VERSION pins the release (e.g. 0.4.0 or v0.4.0); unset means latest.
  version="${DEPEE_VERSION:-}"
  version="${version#v}"

  case "$version" in
    *[!0-9A-Za-z.+-]*)
      printf 'Invalid DEPEE_VERSION: %s\n' "$version" >&2
      exit 1
      ;;
  esac

  os=$(uname -s)
  arch=$(uname -m)

  case "$os/$arch" in
    Darwin/arm64|Darwin/aarch64) target="aarch64-apple-darwin" ;;
    Darwin/x86_64) target="x86_64-apple-darwin" ;;
    Linux/x86_64|Linux/amd64) target="x86_64-unknown-linux-musl" ;;
    Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-musl" ;;
    *)
      printf 'Unsupported platform: %s/%s\n' "$os" "$arch" >&2
      exit 1
      ;;
  esac

  if ! command -v curl >/dev/null 2>&1; then
    printf 'curl is required to download depee.\n' >&2
    exit 1
  fi

  archive="depee-${target}.tar.gz"
  if [ -n "$version" ]; then
    base_url="https://github.com/${repo}/releases/download/v${version}"
    # Match the exact tag, escaping regex metacharacters allowed in versions.
    tag_regex="v$(printf '%s' "$version" | sed 's/[.+]/\\&/g')\$"
  else
    base_url="https://github.com/${repo}/releases/latest/download"
    tag_regex="v"
  fi
  url="${base_url}/${archive}"
  tmp_dir=$(mktemp -d)
  trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

  curl --fail --location --silent --show-error "$url" --output "$tmp_dir/$archive"
  curl --fail --location --silent --show-error "$base_url/SHA256SUMS" --output "$tmp_dir/SHA256SUMS"

  expected=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp_dir/SHA256SUMS")
  if [ -z "$expected" ]; then
    printf 'Checksum for %s not found in SHA256SUMS.\n' "$archive" >&2
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp_dir/$archive" | awk '{ print $1 }')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp_dir/$archive" | awk '{ print $1 }')
  else
    printf 'sha256sum or shasum is required to verify depee.\n' >&2
    exit 1
  fi
  if [ "$actual" != "$expected" ]; then
    printf 'Checksum mismatch for %s.\n' "$archive" >&2
    exit 1
  fi

  if command -v gh >/dev/null 2>&1 && gh auth status --hostname github.com >/dev/null 2>&1; then
    gh attestation verify "$tmp_dir/$archive" --repo "$repo" \
      --hostname github.com \
      --cert-identity-regex "^https://github\\.com/${repo}/\\.github/workflows/release\\.yml@refs/tags/${tag_regex}" \
      >/dev/null
    printf 'Verified build provenance for %s\n' "$archive"
  fi

  tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"

  mkdir -p "$install_dir"
  install -m 755 "$tmp_dir/depee" "$install_dir/depee"

  printf 'Installed depee to %s/depee\n' "$install_dir"
  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *) printf 'Add %s to your PATH to run depee.\n' "$install_dir" ;;
  esac
}

main "$@"
