#!/bin/bash
#
# Re-sign the release binary with webtop's stable identity.
#
# `cargo build` replaces the binary and its signature every time, so this runs
# after every build. Without it the binary reverts to ad-hoc signing and its
# Full Disk Access grant stops matching — see scripts/setup-signing.sh.
#
# A missing identity is not an error: the binary still runs, it just cannot keep
# its TCC permissions across rebuilds.

if [ -z "${BASH_VERSION:-}" ]; then
    exec /bin/bash "$0" "$@"
fi

set -euo pipefail

IDENTITY="webtop code signing"
KEYCHAIN="$HOME/Library/Keychains/webtop-signing.keychain-db"
PASSWORD_FILE="$HOME/.webtop/signing-keychain.pw"
BINARY="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release/webtop"

if [ ! -f "$KEYCHAIN" ] || [ ! -f "$PASSWORD_FILE" ]; then
    echo "No signing keychain — leaving the ad-hoc signature in place."
    echo "Run scripts/setup-signing.sh to stop re-granting Full Disk Access each build."
    exit 0
fi

# The keychain is set never to auto-lock, but a reboot locks it anyway.
if ! security show-keychain-info "$KEYCHAIN" >/dev/null 2>&1; then
    security unlock-keychain -p "$(cat "$PASSWORD_FILE")" "$KEYCHAIN"
fi

# Named explicitly so codesign cannot pick up a stale certificate of the same
# name from another keychain.
if ! codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" \
    --identifier webtop --timestamp=none "$BINARY"; then
    cat >&2 <<'EOF'
error: signing failed; the binary keeps its ad-hoc signature.

  errSecInternalComponent means codesign could not reach the private key.
  Re-run scripts/setup-signing.sh to rebuild the signing keychain.
EOF
    exit 1
fi

requirement="$(codesign -d -r- "$BINARY" 2>/dev/null | sed -n 's/^designated => //p')"
echo "Signed. Code requirement TCC will record:"
echo "  $requirement"
case "$requirement" in
    *"certificate leaf"*)
        echo "  ^ names the certificate, so this survives rebuilds." ;;
    *)
        echo "  warning: expected a 'certificate leaf' requirement." >&2
        echo "  A cdhash requirement breaks on the next build." >&2 ;;
esac
