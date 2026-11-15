# Kick Lite

Linux için Rust ile yazılmış, Kick.com canlı yayınlarını ve yayın tekrarlarını
izlemeye yarayan hafif bir masaüstü istemcisi. Web tarayıcısı gömmez; GTK4,
libadwaita ve libmpv kullanır.

## Özellikler

- Kick kanal adı, kanal URL'si veya VOD URL'si ile açma
- Canlı yayın ve VOD oynatma
- HLS kalite seçimi
- Oynat/duraklat, arama, ses ve tam ekran kontrolleri
- Takip edilen kanalları `~/.config/kick-lite/config.json` içinde saklama
- Wayland ve X11 desteği
- Görsel küçük resimleri ve sohbeti yüklemez; bellek kullanımını düşük tutar

## Gereksinimler

Arch/CachyOS üzerinde:

```bash
sudo pacman -S --needed rust gtk4 libadwaita mpv libepoxy
```

Dağıtımınızda paket adları farklı olabilir. Derleme sırasında `pkg-config`
üzerinden GTK4, libadwaita, libmpv ve OpenGL/EGL geliştirme dosyaları bulunmalıdır.

## Derleme ve çalıştırma

```bash
cargo build --release
./target/release/kick-lite
./target/release/kick-lite xqc
./target/release/kick-lite 'https://kick.com/xqc?video=VOD_ID'
```

Uygulama Kick'in herkese açık, resmî olmayan API uç noktalarını kullanır.
Kick tarafındaki API veya Cloudflare politikası değişirse kanal/VOD bilgisi
alınamayabilir. Canlı kanal listesi için herkese açık ve kararlı bir API
bulunmadığından kanalı adıyla açmak gerekir.

## Kısayollar

| Kısayol | İşlev |
| --- | --- |
| `Space` | Oynat / duraklat |
| `M` | Sesi kapat / aç |
| `F` | Tam ekran |
| `Esc` | Oynatıcıdan geri dön / tam ekrandan çık |
| `←` / `→` | 5 saniye geri / ileri |

## Kurulum

Yerel kullanıcıya kurmak için:

```bash
./install.sh
```

Bu işlem ikiliyi `~/.local/bin`, masaüstü dosyasını
`~/.local/share/applications` ve ikonu `~/.local/share/icons/hicolor/scalable/apps`
altına kopyalar.

## Lisans

MIT
