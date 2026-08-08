pkgname=goat-reuniao
pkgver=0.1.0
pkgrel=1
pkgdesc="Transcrição local de reuniões e push-to-talk de voz (whisper.cpp) - F1/Mouse 4 prompt, F2 reunião"
arch=('x86_64')
url=""
license=('MIT')
depends=('webkit2gtk-4.1' 'gtk3' 'libayatana-appindicator' 'wtype' 'wl-clipboard' 'pipewire-pulse' 'ffmpeg')
makedepends=('cargo')
source=("goat-reuniao.desktop")
sha256sums=('SKIP')

build() {
    cd "$srcdir"
    cargo build --release --manifest-path src-tauri/Cargo.toml
}

package() {
    cd "$srcdir"
    install -Dm755 src-tauri/target/release/goat-reuniao "$pkgdir/usr/bin/goat-reuniao"
    install -Dm755 bin/whisper-server "$pkgdir/usr/lib/goat-reuniao/bin/whisper-server"
    install -Dm755 bin/goat-ipc.sh "$pkgdir/usr/lib/goat-reuniao/bin/goat-ipc.sh"
    install -Dm755 goat-tt-start.sh "$pkgdir/usr/bin/goat-tt-start.sh"
    install -Dm755 goat-tt-stop.sh "$pkgdir/usr/bin/goat-tt-stop.sh"
    install -Dm755 goat-meeting-toggle.sh "$pkgdir/usr/bin/goat-meeting-toggle.sh"
    install -Dm644 models/ggml-large-v3-turbo.bin "$pkgdir/usr/lib/goat-reuniao/models/ggml-large-v3-turbo.bin"
    install -Dm644 src-tauri/icons/icon.png "$pkgdir/usr/share/pixmaps/goat-reuniao.png"
    install -Dm644 goat-reuniao.desktop "$pkgdir/usr/share/applications/goat-reuniao.desktop"
}
