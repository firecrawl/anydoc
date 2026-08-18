#!/usr/bin/env bash
# Cross-compile libanydoc_kotlin.so for Android ABIs and lay them out for the AAR.
# Requires ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) and cargo-ndk.
# Run from the repository root: sh kotlin/scripts/build-android.sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

if [ -z "${ANDROID_NDK_HOME:-}${ANDROID_NDK_ROOT:-}" ]; then
  echo "error: set ANDROID_NDK_HOME to an NDK r28+ install (16 KB page size)." >&2
  exit 1
fi

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_NDK_ROOT}"

# 16 KB ELF alignment: required for Play on 64-bit Android 15+.
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="${CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_RUSTFLAGS="${CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"
export CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS="${CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384"

out="$root/kotlin/android/anydoc/src/main/jniLibs"
mkdir -p "$out"

cargo ndk \
  -t arm64-v8a \
  -t armeabi-v7a \
  -t x86_64 \
  --platform 24 \
  -o "$out" \
  build -p anydoc-kotlin --lib --release

echo "Android native libraries in $out"
found=0
for so in "$out"/*/libanydoc_kotlin.so; do
  [ -f "$so" ] || continue
  found=1
  echo "$so"
  python3 - "$so" <<'PY'
import struct, sys
path = sys.argv[1]
data = open(path, "rb").read()
if data[:4] != b"\x7fELF":
    sys.exit(f"{path}: not ELF")
ei_class = data[4]
if ei_class == 2:
    e_phoff = struct.unpack_from("<Q", data, 32)[0]
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 54)
    p_align_off = 48
    p_align_fmt = "<Q"
elif ei_class == 1:
    e_phoff = struct.unpack_from("<I", data, 28)[0]
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 42)
    p_align_off = 28
    p_align_fmt = "<I"
else:
    sys.exit(f"{path}: unknown ELF class {ei_class}")
align = 0
off = e_phoff
for _ in range(e_phnum):
    p_type = struct.unpack_from("<I", data, off)[0]
    if p_type == 1:  # PT_LOAD
        align = max(align, struct.unpack_from(p_align_fmt, data, off + p_align_off)[0])
    off += e_phentsize
if align < 16384:
    sys.exit(f"{path}: PT_LOAD align {align} < 16384 (16 KB page size)")
print(f"{path}: PT_LOAD align {align}")
PY
done
if [ "$found" -eq 0 ]; then
  echo "error: no libanydoc_kotlin.so produced" >&2
  exit 1
fi
