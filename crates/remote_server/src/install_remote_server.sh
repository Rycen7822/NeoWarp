#!/usr/bin/env bash
# Installs only the binary uploaded for this invocation. Template values are
# single-quoted by setup.rs; they are data, not shell expressions.
set -e

install_dir='{install_dir}'
binary_path='{binary_path}'
staging_binary_path='{staging_binary_path}'
# Use case + prefix removal for bash 3.2 and bash 5.2 compatibility, including
# HOME paths with spaces or '&'. Do not use pattern substitution here.
case "$install_dir" in
  "~"|"~/"*) install_dir="${HOME}${install_dir#\~}" ;;
esac
case "$binary_path" in
  "~"|"~/"*) binary_path="${HOME}${binary_path#\~}" ;;
esac
case "$staging_binary_path" in
  "~"|"~/"*) staging_binary_path="${HOME}${staging_binary_path#\~}" ;;
esac
mkdir -p "$install_dir"

tmpdir=''
cleanup() {
  rm -f "$staging_binary_path" 2>/dev/null || true
  if [ -n "$tmpdir" ]; then
    rm -rf "$tmpdir" 2>/dev/null || true
  fi
}
trap cleanup EXIT

tmpdir=$(mktemp -d "$install_dir/.install.XXXXXX")
if [ ! -f "$staging_binary_path" ] || [ ! -s "$staging_binary_path" ]; then
  echo "error: staged remote server binary is missing or empty" >&2
  exit 1
fi
mv "$staging_binary_path" "$tmpdir/neowarp-server"
chmod +x "$tmpdir/neowarp-server"
# Validate before replacing the current binary. Both paths below are on the
# install filesystem, so the final rename is atomic.
"$tmpdir/neowarp-server" --version >/dev/null
mv "$tmpdir/neowarp-server" "$binary_path"
