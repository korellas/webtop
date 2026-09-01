#!/bin/bash
#
# Create a stable code-signing identity for webtop, so that a Full Disk Access
# grant survives rebuilds.
#
# The problem this solves
# ----------------------
# `cargo build` emits an ad-hoc, linker-signed binary. macOS records TCC grants
# against whatever code requirement the binary has, and an ad-hoc binary has
# only its cdhash:
#
#     kTCCServiceSystemPolicyDesktopFolder   cdhash H"5db8e392..."
#
# The cdhash is a hash of the code. Every rebuild changes it, every grant stops
# matching, and macOS goes back to asking. Worse than asking: an unanswered
# consent dialog blocks `opendir` in the kernel indefinitely, which is what
# froze the folder scanner for two days.
#
# Signing with a certificate instead makes the requirement name the certificate:
#
#     identifier "webtop" and certificate leaf = H"<cert hash>"
#
# That does not change when the code does, so the grant holds across rebuilds.
#
# Why a dedicated keychain
# ------------------------
# Everything here has to work over SSH, where `launchctl managername` reports
# Background rather than Aqua and no Security authorization dialog can be shown.
# Using the login keychain needs its password for `set-key-partition-list`, and
# a keychain whose password does not match the login password — which happens
# after a password change — leaves no way forward at all.
#
# A keychain we create ourselves has a password we chose, so the partition list
# always succeeds and nothing has to be typed. The password is kept at
# PASSWORD_FILE, mode 0600, because `sign.sh` needs it after a reboot.
#
# What that password protects: a self-signed certificate whose only capability
# is signing this one binary so its TCC grant survives rebuilds. It is not a
# distribution identity, Gatekeeper does not trust it, and it can neither
# authorise anything nor sign anything macOS would accept from elsewhere.
#
# Run once. `build.sh` re-signs automatically afterwards.

if [ -z "${BASH_VERSION:-}" ]; then
    exec /bin/bash "$0" "$@"
fi

set -euo pipefail

if [ "$(id -u)" -eq 0 ]; then
    echo "error: do not run this with sudo — the identity must be yours." >&2
    exit 1
fi

IDENTITY="webtop code signing"
KEYCHAIN="$HOME/Library/Keychains/webtop-signing.keychain-db"
PASSWORD_FILE="$HOME/.webtop/signing-keychain.pw"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$REPO/target/release/webtop"

# LibreSSL (/usr/bin/openssl) cannot write a PKCS#12 that macOS will import.
OPENSSL=/opt/homebrew/bin/openssl
[ -x "$OPENSSL" ] || OPENSSL=/usr/local/opt/openssl@3/bin/openssl
if [ ! -x "$OPENSSL" ]; then
    echo "error: need OpenSSL 3 (brew install openssl@3); LibreSSL will not do." >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Earlier attempts against the login keychain leave a certificate and key that
# codesign cannot use. Left in place they would make `find-certificate -c`
# ambiguous, and sign.sh could pick the dead one.
if security find-certificate -c "$IDENTITY" "$HOME/Library/Keychains/login.keychain-db" \
    >/dev/null 2>&1; then
    echo "Removing the unusable '$IDENTITY' left in your login keychain..."
    while security delete-identity -c "$IDENTITY" \
        "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1; do :; done
fi

echo "Creating a dedicated signing keychain..."
mkdir -p "$(dirname "$PASSWORD_FILE")"
KC_PW="$("$OPENSSL" rand -base64 24)"
( umask 077; printf '%s' "$KC_PW" > "$PASSWORD_FILE" )

security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
security create-keychain -p "$KC_PW" "$KEYCHAIN"
# No -l and no -u: never auto-lock. sign.sh unlocks explicitly after a reboot.
security set-keychain-settings "$KEYCHAIN"
security unlock-keychain -p "$KC_PW" "$KEYCHAIN"

cat > "$WORK/req.cnf" <<'EOF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3

[dn]
CN = webtop code signing

[v3]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
EOF

echo "Generating a self-signed code-signing certificate (20 years)..."
"$OPENSSL" req -x509 -newkey rsa:2048 -sha256 -days 7300 -nodes \
    -keyout "$WORK/key.pem" -out "$WORK/cert.pem" -config "$WORK/req.cnf" 2>/dev/null

P12_PW="$("$OPENSSL" rand -hex 16)"
"$OPENSSL" pkcs12 -export -legacy -macalg sha1 \
    -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
    -name "$IDENTITY" -out "$WORK/bundle.p12" -passout "pass:$P12_PW"

echo "Importing it..."
security import "$WORK/bundle.p12" -k "$KEYCHAIN" -P "$P12_PW" -T /usr/bin/codesign -A

# The step that could not be done against the login keychain. Here the password
# is one we generated a moment ago, so it cannot be wrong.
echo "Allowing codesign to use the key without prompting..."
security set-key-partition-list -S apple-tool:,apple: -s -k "$KC_PW" "$KEYCHAIN" >/dev/null

# codesign only searches keychains on the search list. Rebuild it rather than
# replace it: `list-keychains -s` overwrites, and dropping the login keychain
# would break every other tool on the machine.
echo "Adding it to the keychain search list..."
current="$(security list-keychains -d user | sed 's/[",]//g' | xargs)"
in_list=false
for kc in $current; do
    [ "$kc" = "$KEYCHAIN" ] && in_list=true
done
if [ "$in_list" = false ]; then
    # shellcheck disable=SC2086
    security list-keychains -d user -s $current "$KEYCHAIN"
fi

# Trust is best effort and not required for what we are after. It governs
# Gatekeeper and whether `find-identity -v` lists the identity; the code
# requirement TCC records names the certificate by hash and involves no chain
# validation. It also cannot be set from a session with no GUI.
if ! sudo -n true 2>/dev/null; then
    echo "Skipping Gatekeeper trust (needs sudo and an Aqua session; not required here)."
elif ! sudo security add-trusted-cert -d -r trustRoot -p codeSign \
    -k /Library/Keychains/System.keychain "$WORK/cert.pem" 2>/dev/null; then
    echo "Gatekeeper trust not set — expected over SSH, and not required here."
fi

if [ ! -f "$BINARY" ]; then
    echo "No binary at $BINARY yet — run ./build.sh, which will sign it."
    exit 0
fi

"$REPO/scripts/sign.sh"

cat <<EOF

One manual step remains, and only you can do it — it needs the GUI:

  System Settings -> Privacy & Security -> Full Disk Access
  -> "+" -> Shift-Cmd-G -> paste:

      $BINARY

  -> add it, and make sure its switch is ON.

Then restart webtop:

  sudo launchctl kickstart -k system/com.webtop

Why Full Disk Access rather than six folder permissions: the scanner's whole
job is measuring what is on the disk, and TCC-protected directories do not
merely refuse it — they block it. One grant covers Desktop, Documents,
Downloads, Music, Pictures, Movies and the 764 app containers at once.
EOF
