pkgname=goat-reuniao
pkgver=0.1.0
pkgrel=1
pkgdesc="Transcrição local de reuniões e push-to-talk via whisper.cpp — Mouse 4/F1 prompt, F2 reunião"
arch=('x86_64')
url="https://github.com/luite/goat-reuniao"
license=('MIT')
depends=('webkit2gtk-4.1' 'gtk3' 'libayatana-appindicator' 'wtype' 'wl-clipboard'
         'pipewire-pulse' 'ffmpeg')
makedepends=('cargo')
install=goat-reuniao.install
source=("$pkgname::git+file://$startdir"
        "goat-reuniao.desktop")
sha256sums=('SKIP' 'SKIP')

_model_url="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
_model_sha="30eed2485cb740db93743ec7652949741790f10592a26e12e34c5865187e1511"

prepare() {
    cd "$pkgname"
    export RUSTUP_TOOLCHAIN=stable
    cargo fetch --locked --manifest-path src-tauri/Cargo.toml
}

build() {
    cd "$pkgname"
    cargo build --release --frozen --manifest-path src-tauri/Cargo.toml

    # Baixar modelo se não existir no source local
    mkdir -p models
    if [ ! -f models/ggml-large-v3-turbo.bin ]; then
        echo ">>> Baixando modelo whisper large-v3-turbo (~1.6GB)..."
        curl -fSL -o models/ggml-large-v3-turbo.bin "$_model_url"
    fi
}

package() {
    cd "$pkgname"

    install -Dm755 src-tauri/target/release/goat-reuniao \
        "$pkgdir/usr/bin/goat-reuniao"
    install -Dm755 bin/whisper-server \
        "$pkgdir/usr/lib/goat-reuniao/bin/whisper-server"
    install -Dm755 bin/goat-ipc.sh \
        "$pkgdir/usr/lib/goat-reuniao/bin/goat-ipc.sh"
    install -Dm755 goat-tt-start.sh \
        "$pkgdir/usr/bin/goat-tt-start.sh"
    install -Dm755 goat-tt-stop.sh \
        "$pkgdir/usr/bin/goat-tt-stop.sh"
    install -Dm755 goat-meeting-toggle.sh \
        "$pkgdir/usr/bin/goat-meeting-toggle.sh"
    install -Dm644 src-tauri/icons/icon.png \
        "$pkgdir/usr/share/pixmaps/goat-reuniao.png"
    install -Dm644 goat-reuniao.desktop \
        "$pkgdir/usr/share/applications/goat-reuniao.desktop"

    # Modelo: instalar se disponível, senão mensagem pós-install
    if [ -f models/ggml-large-v3-turbo.bin ]; then
        install -Dm644 models/ggml-large-v3-turbo.bin \
            "$pkgdir/usr/lib/goat-reuniao/models/ggml-large-v3-turbo.bin"
    fi
}
