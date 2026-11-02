#!/usr/bin/env bash
set -euo pipefail

REPO="EnricoDiMaria/lapis"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
DATA_DIR="$HOME/.local/share/lapis"
MODEL_DIR="$DATA_DIR/models/parakeet-tdt-0.6b-v3-int8"
BASE_MODEL_URL="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
RELEASE_URL="https://github.com/$REPO/releases/latest/download/lapis-linux-x86_64.tar.gz"
LOCAL_ARCHIVE="${2:-./lapis-linux-x86_64.tar.gz}"

CURL_OPTS=(-fL --retry 5 --retry-delay 2 --retry-all-errors --connect-timeout 15)

TMP_FILES=()
cleanup() {
    for f in "${TMP_FILES[@]:-}"; do
        [[ -n "$f" ]] && rm -f "$f"
    done
}
trap cleanup EXIT

need() {
    command -v "$1" >/dev/null 2>&1 || { echo "✖ Error: '$1' is required but not installed."; exit 1; }
}

ask() {
    local prompt="$1" ans=""
    if [[ -r /dev/tty ]]; then
        read -r -p "$prompt" -n 1 ans < /dev/tty || true
        echo "" >&2
    fi
    REPLY="$ans"
}

check_dns() {
    local host="$1"
    if command -v getent >/dev/null 2>&1; then
        if ! getent hosts "$host" >/dev/null 2>&1; then
            echo "⚠ Cannot resolve $host right now (DNS issue). Retrying automatically while downloading..."
        fi
    fi
}

download_file() {
    local url="$1" dest="$2" part="$2.part"
    echo "   $(basename "$dest")"
    curl "${CURL_OPTS[@]}" -C - --progress-bar "$url" -o "$part"
    if [[ ! -s "$part" ]]; then
        echo "✖ Error: downloaded file is empty: $dest"
        rm -f "$part"
        exit 1
    fi
    mv -f "$part" "$dest"
}

install_binary() {
    mkdir -p "$BIN_DIR"
    need tar

    if [[ "${1:-}" == "local" || -f "$LOCAL_ARCHIVE" ]]; then
        if [[ -f "$LOCAL_ARCHIVE" ]]; then
            echo "-> Installing Lapis from local archive: $LOCAL_ARCHIVE..."
            tar -xzf "$LOCAL_ARCHIVE" -C "$BIN_DIR"
        elif [[ -f "./target/release/lapis" ]]; then
            echo "-> Installing Lapis from local build: ./target/release/lapis..."
            cp "./target/release/lapis" "$BIN_DIR/lapis"
        else
            echo "✖ Error: Neither '$LOCAL_ARCHIVE' nor './target/release/lapis' found locally."
            exit 1
        fi
    else
        need curl
        echo "-> Downloading latest Lapis binary from GitHub..."
        check_dns github.com
        local tmp
        tmp="$(mktemp)"
        TMP_FILES+=("$tmp")
        curl "${CURL_OPTS[@]}" --progress-bar "$RELEASE_URL" -o "$tmp"
        tar -xzf "$tmp" -C "$BIN_DIR"
    fi

    if [[ ! -f "$BIN_DIR/lapis" ]]; then
        echo "✖ Error: $BIN_DIR/lapis not found after extraction (unexpected archive layout?)."
        exit 1
    fi
    chmod +x "$BIN_DIR/lapis"
    echo "✔ Binary installed in $BIN_DIR/lapis"
}

install_desktop() {
    echo "-> Registering icon and desktop entry..."
    mkdir -p "$APP_DIR" "$ICON_DIR"

    cat > "$ICON_DIR/lapis.svg" << 'EOF'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <rect width="256" height="256" rx="56" fill="#181825"/>
  <path d="M92.69 216H48a8 8 0 0 1-8-8v-44.69a8 8 0 0 1 2.34-5.65l120-120a8 8 0 0 1 11.31 0l44.69 44.69a8 8 0 0 1 0 11.31l-120 120A8 8 0 0 1 92.69 216Z" fill="none" stroke="#89b4fa" stroke-linecap="round" stroke-linejoin="round" stroke-width="16"/>
  <line x1="136" y1="64" x2="192" y2="120" stroke="#89b4fa" stroke-linecap="round" stroke-linejoin="round" stroke-width="16"/>
</svg>
EOF

    cat > "$APP_DIR/lapis.desktop" << EOF
[Desktop Entry]
Version=1.1.0
Type=Application
Name=Lapis
GenericName=PDF Study Suite
Comment=Advanced PDF study suite with audio sync, vector drawing & Markdown notes
Exec=$BIN_DIR/lapis %f
Icon=lapis
Terminal=false
Categories=Office;Education;Viewer;
MimeType=application/pdf;
Keywords=pdf;notes;study;markdown;latex;lecture;
StartupWMClass=lapis
EOF

    update-desktop-database "$APP_DIR" 2>/dev/null || true
    echo "✔ Desktop entry registered"
}

download_model() {
    need curl
    echo "-> Downloading Parakeet TDT 0.6B v3 (int8) ASR model (~650 MB)..."
    echo "   (interrupted downloads resume automatically; just re-run)"
    mkdir -p "$MODEL_DIR"
    check_dns huggingface.co

    local files=(vocab.txt encoder-model.int8.onnx decoder_joint-model.int8.onnx)
    local f
    for f in "${files[@]}"; do
        if [[ -s "$MODEL_DIR/$f" && ! -f "$MODEL_DIR/$f.part" ]]; then
            echo "   $f already present, skipping"
            continue
        fi
        download_file "$BASE_MODEL_URL/$f" "$MODEL_DIR/$f"
    done
    echo "✔ Parakeet model installed in $MODEL_DIR"
}

uninstall_lapis() {
    echo "-> Uninstalling Lapis..."
    rm -f "$BIN_DIR/lapis" "$APP_DIR/lapis.desktop" "$ICON_DIR/lapis.svg"
    update-desktop-database "$APP_DIR" 2>/dev/null || true
    echo "✔ Lapis binary and desktop entry removed."
    ask "Do you also want to remove downloaded ASR models and dictionaries in ~/.local/share/lapis? [y/N] "
    if [[ "$REPLY" =~ ^[Yy]$ ]]; then
        rm -rf "$DATA_DIR"
        echo "✔ Removed ~/.local/share/lapis"
    fi
}

show_help() {
    cat << EOF
Usage: ./install.sh [COMMAND] [ARCHIVE_PATH]

Commands:
  install             Install Lapis (uses local .tar.gz if present, otherwise GitHub)
  local [path.tar.gz] Install or update strictly from a local archive or ./target/release/lapis
  update              Update the Lapis binary from GitHub (or local if present)
  model               Download or resume the Parakeet offline ASR model
  uninstall           Remove Lapis from your system
  help                Show this help message
EOF
}

ACTION="${1:-}"

case "$ACTION" in
    local|--local|-l)
        echo "󰏪 Installing Lapis from local package..."
        install_binary "local"
        install_desktop
        ;;
    update|--update|-u)
        echo "󰏪 Updating Lapis..."
        install_binary
        ;;
    model|download-model|--model|-m)
        download_model
        ;;
    uninstall|--uninstall)
        uninstall_lapis
        ;;
    help|--help|-h)
        show_help
        ;;
    install|--install|"")
        if [[ -f "$BIN_DIR/lapis" && -z "$ACTION" ]]; then
            echo "󰏪 Lapis is already installed. What would you like to do?"
            echo "  1) Update Lapis binary (uses local .tar.gz if in current folder, else GitHub)"
            echo "  2) Install from local ./lapis-linux-x86_64.tar.gz or ./target/release/lapis"
            echo "  3) Download / Repair Parakeet ASR model (~650 MB)"
            echo "  4) Reinstall everything (binary + .desktop + icon)"
            echo "  5) Uninstall Lapis"
            echo "  6) Cancel"
            ask "Select an option [1-6]: "
            case "$REPLY" in
                1) install_binary ;;
                2) install_binary "local"; install_desktop ;;
                3) download_model ;;
                4) install_binary; install_desktop ;;
                5) uninstall_lapis ;;
                *) echo "Cancelled."; exit 0 ;;
            esac
        else
            echo "󰏪 Installing Lapis..."
            install_binary
            install_desktop
            echo ""
            if [[ ! -s "$MODEL_DIR/encoder-model.int8.onnx" || -f "$MODEL_DIR/encoder-model.int8.onnx.part" ]]; then
                ask "Do you also want to download the Parakeet offline ASR model (~650 MB)? [y/N] "
                if [[ "$REPLY" =~ ^[Yy]$ ]]; then
                    download_model
                fi
            fi
            echo ""
            echo "✔ Lapis installed successfully! Make sure $BIN_DIR is in your PATH."
        fi
        ;;
    *)
        if [[ -f "$ACTION" ]]; then
            LOCAL_ARCHIVE="$ACTION"
            echo "󰏪 Installing Lapis from $LOCAL_ARCHIVE..."
            install_binary "local"
            install_desktop
        else
            echo "Unknown command or missing file: $ACTION"
            show_help
            exit 1
        fi
        ;;
esac
