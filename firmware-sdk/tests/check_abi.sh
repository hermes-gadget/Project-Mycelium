#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-${repo_root}/target}"
profile_dir="${target_dir}/debug"
test_binary="${target_dir}/firmware-sdk-abi-link"

cd "${repo_root}"
cargo build -p meshemu_bridge
"${CC:-cc}" \
    -std=c11 \
    -Wall \
    -Wextra \
    -Werror \
    -Ifirmware-sdk/include \
    -Icore/bridge/include \
    firmware-sdk/tests/abi_link.c \
    -L"${profile_dir}" \
    -Wl,-rpath,"${profile_dir}" \
    -lmeshemu_bridge \
    -o "${test_binary}"
"${test_binary}"
