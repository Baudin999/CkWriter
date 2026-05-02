#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_NAME="CkWriter"
DEST_DIR="${HOME}/.local/bin"
DEST="${DEST_DIR}/${BIN_NAME}"
APPS_DIR="${HOME}/.local/share/applications"
DESKTOP="${APPS_DIR}/ckwriter.desktop"

cd "${SCRIPT_DIR}"

echo "→ building release binary"
cargo build --release

echo "→ installing binary to ${DEST}"
mkdir -p "${DEST_DIR}"
install -m 0755 target/release/ckwriter "${DEST}"

echo "→ writing desktop entry to ${DESKTOP}"
mkdir -p "${APPS_DIR}"
cat > "${DESKTOP}" <<EOF
[Desktop Entry]
Name=CkWriter
Comment=Local LLM-coached novel writing
Exec=${DEST}
Terminal=false
Type=Application
Icon=accessories-text-editor
Categories=Office;WordProcessor;Utility;
EOF

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APPS_DIR}" 2>/dev/null || true
fi

echo "✓ installed"

case ":${PATH}:" in
    *":${DEST_DIR}:"*) ;;
    *)
        echo
        echo "⚠ ${DEST_DIR} is not in your PATH."
        echo "  add this to your shell rc:"
        echo "    export PATH=\"\${HOME}/.local/bin:\${PATH}\""
        ;;
esac

echo
echo "run with:  ${BIN_NAME}    (or pick CkWriter from your app menu)"
