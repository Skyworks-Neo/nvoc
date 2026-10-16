#!/bin/sh
# probe-macos-rm build script.
#
# The suite compiles against NVIDIA's open-gpu-kernel-modules (OGKM) SDK
# headers — the same tree the NullMoth driver itself builds from, so every
# struct/constant in the Tier-A battery is the authoritative one by
# construction. Cloning it is one command (see README).
#
#   sh build.sh             # build ./probe-macos-rm-idle (no GPU workloads)
#   PROFILE=full sh build.sh # unrestricted suite, for suitable hardware only
#   LINT=1 ./build.sh       # off-target lint build (IOKit stubbed; needs any
#                           # POSIX C compiler, not just clang)
set -e
cd "$(dirname "$0")"

OGKM="${OGKM:-$HOME/ogkm610}"
if [ ! -d "$OGKM/src/common/sdk/nvidia/inc" ]; then
    echo "error: OGKM tree not found at $OGKM" >&2
    echo "  git clone --depth 1 --branch 610.57.04 \\" >&2
    echo "       https://github.com/NVIDIA/open-gpu-kernel-modules ~/ogkm610" >&2
    exit 1
fi

SDK="$OGKM/src/common/sdk/nvidia/inc"
UNIX="$OGKM/src/nvidia/arch/nvalloc/unix/include"

CC="${CC:-cc}"
CFLAGS="-std=c11 -O2 -Wall -Wextra -Wno-unused-parameter"
PROFILE="${PROFILE:-idle}"
case "$PROFILE" in
    idle) IDLE_ONLY=1; OUT="${OUT:-probe-macos-rm-idle}" ;;
    full) IDLE_ONLY=0; OUT="${OUT:-probe-macos-rm}" ;;
    *) echo "error: PROFILE must be idle or full" >&2; exit 2 ;;
esac
set -- "-I$SDK" "-I$UNIX" "-DNVX_IDLE_ONLY=$IDLE_ONLY"

SRCS="src/main.c src/nvx_xnu.c src/nvx_rm.c src/nvx_suite.c"

if [ "${LINT:-0}" = "1" ]; then
    # Off-target syntax/type check: IOKit layer compiles as stubs.
    "$CC" $CFLAGS -DNVX_LINT -fsyntax-only "$@" $SRCS
    echo "lint build OK"
    exit 0
fi

case "$(uname -s)" in
    Darwin) ;;
    *) echo "error: real build requires macOS (IOKit); use LINT=1 elsewhere" >&2; exit 1 ;;
esac

"$CC" $CFLAGS "$@" $SRCS -o "$OUT" -framework IOKit -framework CoreFoundation
echo "built $OUT (profile $PROFILE)"
