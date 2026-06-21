#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/macos/install.sh [options]

Build parakit from source and install a self-contained macOS CLI layout:

  <prefix>/bin/parakit
  <prefix>/lib/parakit/*.dylib

Options:
  --prefix DIR        Install prefix. Default: $PARAKIT_PREFIX, $CARGO_HOME, or ~/.cargo.
  --features LIST     Cargo features. Default: $PARAKIT_FEATURES or metal.
  --debug             Build and install the debug profile instead of release.
  --locked            Pass --locked to cargo build.
  --skip-build        Install from an existing target/<profile>/parakit.
  -h, --help          Show this help.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

if [ "$(uname -s)" != "Darwin" ]; then
  die "scripts/macos/install.sh is only for macOS"
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
prefix="${PARAKIT_PREFIX:-${CARGO_HOME:-$HOME/.cargo}}"
features="${PARAKIT_FEATURES:-metal}"
profile="release"
cargo_profile_flag=(--release)
locked=()
skip_build=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] || die "--prefix requires a directory"
      prefix="$2"
      shift 2
      ;;
    --features)
      [ "$#" -ge 2 ] || die "--features requires a feature list"
      features="$2"
      shift 2
      ;;
    --debug)
      profile="debug"
      cargo_profile_flag=()
      shift
      ;;
    --locked)
      locked=(--locked)
      shift
      ;;
    --skip-build)
      skip_build=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

cd "$repo_root"

if [ "$skip_build" -eq 0 ]; then
  build_cmd=(cargo build "${cargo_profile_flag[@]}" "${locked[@]}")
  if [ -n "$features" ]; then
    build_cmd+=(--features "$features")
  fi
  printf '+'
  printf ' %q' "${build_cmd[@]}"
  printf '\n'
  "${build_cmd[@]}"
fi

built_bin="$repo_root/target/$profile/parakit"
[ -x "$built_bin" ] || die "missing built binary: $built_bin"

rpaths_for() {
  otool -l "$1" | awk '
    /LC_RPATH/ { in_rpath = 1; next }
    in_rpath && $1 == "path" { print $2; in_rpath = 0 }
  '
}

runtime_lib_dir=""
while IFS= read -r rpath; do
  case "$rpath" in
    @*) continue ;;
  esac
  if [ -f "$rpath/libcrispasr.dylib" ]; then
    runtime_lib_dir="$rpath"
    break
  fi
done < <(rpaths_for "$built_bin")

[ -n "$runtime_lib_dir" ] || die "could not find CrispASR runtime dylib directory from $built_bin rpath"
[ -d "$runtime_lib_dir" ] || die "runtime dylib directory does not exist: $runtime_lib_dir"

bin_dir="$prefix/bin"
lib_dir="$prefix/lib/parakit"
tmp_lib_dir="$prefix/lib/.parakit.tmp.$$"
installed_bin="$bin_dir/parakit"
install_rpath="@executable_path/../lib/parakit"

mkdir -p "$bin_dir" "$prefix/lib"
rm -rf "$tmp_lib_dir"
mkdir -p "$tmp_lib_dir"

find "$runtime_lib_dir" -maxdepth 1 \( -type f -o -type l \) -name '*.dylib' -exec cp -P {} "$tmp_lib_dir/" \;
[ -f "$tmp_lib_dir/libcrispasr.dylib" ] || die "libcrispasr.dylib was not copied from $runtime_lib_dir"

rm -rf "$lib_dir"
mv "$tmp_lib_dir" "$lib_dir"
install -m 0755 "$built_bin" "$installed_bin"

while IFS= read -r rpath; do
  if [ "$rpath" = "$runtime_lib_dir" ]; then
    install_name_tool -delete_rpath "$rpath" "$installed_bin"
  fi
done < <(rpaths_for "$installed_bin")

if ! rpaths_for "$installed_bin" | grep -qxF "$install_rpath"; then
  install_name_tool -add_rpath "$install_rpath" "$installed_bin"
fi

if command -v codesign >/dev/null 2>&1; then
  codesign --force --sign - "$installed_bin" >/dev/null
fi

if rpaths_for "$installed_bin" | grep -q "$repo_root/target/"; then
  die "installed binary still contains a repository target/ rpath"
fi

printf 'installed %s\n' "$installed_bin"
printf 'installed runtime libraries in %s\n' "$lib_dir"
printf 'verify with: %s --verbose doctor\n' "$installed_bin"
