#!/usr/bin/env bash
set -euo pipefail

target_dir="${1:?Pass the bundled Python directory}"
version="3.12.14"
license_hash="3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf"
license_url="https://raw.githubusercontent.com/python/cpython/v${version}/LICENSE"

mkdir -p "$target_dir"
curl --fail --show-error --location --retry 3 "$license_url" -o "$target_dir/PYTHON-LICENSE"
echo "$license_hash  $target_dir/PYTHON-LICENSE" | shasum -a 256 --check
