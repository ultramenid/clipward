#!/usr/bin/env bash
# Build Clipward's GPUI app (crates/app) and assemble target/bundle/Clipward.app,
# the macOS app bundle.
#
#   scripts/bundle-macos.sh [--debug] [--no-build] [--features <list>] [--dmg] [--no-sign]
#
#   --debug           bundle target/debug/clipward-app instead of a release build
#   --no-build        package the binary already in target/<profile>/ (no cargo run)
#   --features <list> passed to cargo build (e.g. snapshot for a headless check)
#   --dmg             also write target/bundle/Clipward_<version>_<arch>.dmg
#   --no-sign         leave the bundle unsigned
#
# Signing and notarization read:
#   APPLE_SIGNING_IDENTITY  Developer ID Application identity in the keychain.
#                           Unset: ad-hoc signature (-), runnable on this Mac only.
#   APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
#     or APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH (App Store Connect key)
#                           Set with a signing identity: notarize and staple.
#
# Bundle layout (what AppPaths resolves at runtime):
#   Contents/MacOS/clipward
#   Contents/Info.plist                    crates/app/macos/Info.plist
#   Contents/Resources/icon.icns           crates/app/icons/icon.icns
#   Contents/Resources/engine-bin/         engine-bin/ (ffmpeg, ffprobe, yt-dlp, dylibs)
#   Contents/Resources/assets/fonts/       crates/engine/assets/fonts/
#   Contents/Resources/assets/models/      crates/engine/assets/models/
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
profile=release
build=1
features=""
make_dmg=0
sign=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug) profile=debug ;;
    --no-build) build=0 ;;
    --features) features="${2:?--features needs a list}"; shift ;;
    --dmg) make_dmg=1 ;;
    --no-sign) sign=0 ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

[[ "$(uname -s)" == "Darwin" ]] || { echo "bundle-macos.sh runs on macOS only" >&2; exit 1; }

target_dir="${CARGO_TARGET_DIR:-$root/target}"
binary="$target_dir/$profile/clipward-app"
out_dir="$target_dir/bundle"
app="$out_dir/Clipward.app"
macos_dir="$root/crates/app/macos"
exe_name=clipward

version="$(awk '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && /^version[[:space:]]*=/ { gsub(/.*=[[:space:]]*"|".*/, ""); print; exit }
' "$root/crates/app/Cargo.toml")"
[[ -n "$version" ]] || { echo "No version in crates/app/Cargo.toml" >&2; exit 1; }

# ---- Build -------------------------------------------------------------------
if [[ "$build" == 1 ]]; then
  cargo_args=(build -p clipward_app --bin clipward-app)
  [[ "$profile" == release ]] && cargo_args+=(--release)
  [[ -n "$features" ]] && cargo_args+=(--features "$features")
  echo "==> cargo ${cargo_args[*]}"
  (cd "$root" && cargo "${cargo_args[@]}")
fi
[[ -x "$binary" ]] || { echo "Missing $binary (build it or drop --no-build)" >&2; exit 1; }

# ---- Resources ---------------------------------------------------------------
# engine-bin/ -> engine-bin/,
# ../crates/engine/assets/{fonts,models}/ -> assets/{fonts,models}/.
engine_bin="$root/engine-bin"
for tool in ffmpeg ffprobe; do
  if [[ ! -x "$engine_bin/$tool" ]]; then
    if [[ "$profile" == release ]]; then
      echo "engine-bin/$tool is missing. Stage the media tools first (docs/RELEASING.md)." >&2
      exit 1
    fi
    echo "warning: engine-bin/$tool is missing; the bundle will report missing tools" >&2
  fi
done
[[ -x "$engine_bin/yt-dlp" ]] || echo "warning: engine-bin/yt-dlp is missing" >&2

# ---- Assemble ----------------------------------------------------------------
echo "==> Assembling $app ($profile, v$version)"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/assets"

cp "$binary" "$app/Contents/MacOS/$exe_name"
chmod 755 "$app/Contents/MacOS/$exe_name"
cp "$root/crates/app/icons/icon.icns" "$app/Contents/Resources/icon.icns"
if [[ -d "$engine_bin" ]]; then
  ditto "$engine_bin" "$app/Contents/Resources/engine-bin"
else
  mkdir -p "$app/Contents/Resources/engine-bin"
fi
ditto "$root/crates/engine/assets/fonts" "$app/Contents/Resources/assets/fonts"
ditto "$root/crates/engine/assets/models" "$app/Contents/Resources/assets/models"
find "$app" -name .DS_Store -delete

# Deployment target of the binary (arm64 builds: 11.0), never below 10.13.
min_os="$(otool -l "$binary" | awk '
  /cmd LC_BUILD_VERSION/ { build = 1 } build && $1 == "minos" { print $2; exit }
  /cmd LC_VERSION_MIN_MACOSX/ { legacy = 1 } legacy && $1 == "version" { print $2; exit }
')"
min_os="$(printf '%s\n%s\n' "${min_os:-10.13}" 10.13 | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"

plist="$app/Contents/Info.plist"
cp "$macos_dir/Info.plist" "$plist"
plutil -replace CFBundleExecutable -string "$exe_name" "$plist"
plutil -replace CFBundleShortVersionString -string "$version" "$plist"
plutil -replace CFBundleVersion -string "$version" "$plist"
plutil -replace LSMinimumSystemVersion -string "$min_os" "$plist"
plutil -lint "$plist" >/dev/null

# ---- Sign --------------------------------------------------------------------
identity="${APPLE_SIGNING_IDENTITY:-}"
entitlements="$macos_dir/entitlements.plist"

macho_files() {
  find "$app/Contents/Resources" -type f -print0 |
    while IFS= read -r -d '' file; do
      if [[ "$(file -b "$file")" == Mach-O* ]]; then printf '%s\0' "$file"; fi
    done
}

# codesign, minus the "replacing existing signature" note; keeps its exit status.
sign_code() {
  codesign --force "$@" 2>&1 | sed '/replacing existing signature/d'
}

if [[ "$sign" == 1 ]]; then
  if [[ -n "$identity" ]]; then
    echo "==> Signing with \"$identity\" (hardened runtime)"
    # Inside out: the media tools and their dylibs, then the app. Same team ID,
    # so ffmpeg's hardened runtime still loads its @loader_path dylibs.
    macho_files | while IFS= read -r -d '' file; do
      sign_code --timestamp --options runtime --sign "$identity" "$file"
    done
    sign_code --timestamp --options runtime \
      --entitlements "$entitlements" --sign "$identity" "$app"
  else
    echo "==> APPLE_SIGNING_IDENTITY unset: ad-hoc signing (runs on this Mac only)"
    # Ad-hoc media tools keep plain signatures (as stage-ffmpeg-libs-mac.sh leaves
    # them): ad-hoc code has no team ID, so library validation would refuse the dylibs.
    macho_files | while IFS= read -r -d '' file; do
      sign_code --sign - "$file"
    done
    sign_code --options runtime --entitlements "$entitlements" --sign - "$app"
  fi
  codesign --verify --strict --deep "$app"
fi

# ---- Notarize ----------------------------------------------------------------
notary_args=()
if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
  notary_args=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  notary_args=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
fi

notarize() { # <file to submit> <file to staple>
  echo "==> Notarizing $(basename "$2")"
  local result status id
  result="$(xcrun notarytool submit "$1" "${notary_args[@]}" --wait --output-format json)"
  status="$(plutil -extract status raw -o - - <<<"$result")"
  id="$(plutil -extract id raw -o - - <<<"$result")"
  if [[ "$status" != Accepted ]]; then
    echo "Notarization $status (submission $id). Details:" >&2
    xcrun notarytool log "$id" "${notary_args[@]}" >&2 || true
    exit 1
  fi
  xcrun stapler staple "$2"
}

can_notarize=0
if [[ "$sign" == 1 && -n "$identity" && ${#notary_args[@]} -gt 0 ]]; then
  can_notarize=1
  zip="$out_dir/Clipward-notarize.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"
  notarize "$zip" "$app"
  rm -f "$zip"
elif [[ -n "$identity" ]]; then
  echo "==> Notarization skipped: set APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID or APPLE_API_KEY/APPLE_API_ISSUER/APPLE_API_KEY_PATH"
fi

# ---- DMG ---------------------------------------------------------------------
if [[ "$make_dmg" == 1 ]]; then
  arch="$(lipo -archs "$binary" | tr ' ' '_')"
  [[ "$arch" == arm64 ]] && arch=aarch64
  [[ "$arch" == x86_64 ]] && arch=x64
  dmg="$out_dir/Clipward_${version}_${arch}.dmg"
  staging="$(mktemp -d)"
  trap 'rm -rf "$staging"' EXIT
  ditto "$app" "$staging/Clipward.app"
  ln -s /Applications "$staging/Applications"
  rm -f "$dmg"
  echo "==> Writing $dmg"
  hdiutil create -quiet -volname Clipward -srcfolder "$staging" -fs HFS+ -format UDZO -ov "$dmg"
  if [[ "$sign" == 1 && -n "$identity" ]]; then
    sign_code --timestamp --sign "$identity" "$dmg"
    [[ "$can_notarize" == 1 ]] && notarize "$dmg" "$dmg"
  fi
fi

echo "==> Done: $app"
