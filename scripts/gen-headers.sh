#!/bin/sh
# Regenerate go/include/anydoc.h from the crate's C ABI.
#
# Run from the repo root:
#   sh scripts/gen-headers.sh
#
# The header is committed; CI verifies it is up to date. End users who
# download the prebuilt libanydoc_go.a never run this.
set -e

cargo build -p anydoc-go --release

if [ ! -f go/include/anydoc.h ]; then
  echo "error: go/include/anydoc.h was not generated" >&2
  exit 1
fi

echo "wrote go/include/anydoc.h"