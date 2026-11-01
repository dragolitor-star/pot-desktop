# Yerel web çalışma alanı

Güncel geliştirme durumu, test kanıtları ve yeni bilgisayarda devam komutları:
[1 Kasım 2026 devir promptu](../DEVIR-WEB-2026-11-01.md). Web taşınması henüz
tüm ekranlarda uçtan uca doğrulanmış bir son sürüm değildir.

Uygulama `127.0.0.1` üzerinde rastgele portlu sunucu başlatır ve varsayılan
tarayıcıda oturuma özel adresi açar. Sistem tepsisindeki **Web arayüzünü aç**
aynı oturumu tekrar açar. Yerel Rust uygulaması açık kalmalıdır. Sunucu başka
bilgisayarlara veya internete açılmaz.

Ana giriş `workspace.html`, React shell `src/browser/Workspace.jsx` dosyasıdır.
Mevcut masaüstü metin/OCR/belge/medya, servis ve eklenti, sözlük, geçmiş,
ayarlar, kısayol ve yedekleme ekranları aynı bileşenlerle tarayıcıda sunulur.
Tauri komutları doğrulanmış HTTP köprüsünden görünmez yerel worker'a gider.
Streaming sağlayıcı çağrıları native HTTP üzerinden aktarılır. Ekran bölgesi
seçimi ve isteğe bağlı canlı altyazı overlay'i yerel pencere olarak çalışır.
Önceki Gemini ağırlıklı hızlı arayüz `/quick` yolunda korunur.

Workspace masaüstüyle aynı yerel config store, SQLite geçmişi, sözlük ve
cache'i kullanır. Servis yapılandırmasında kaydedilen API anahtarları mevcut
masaüstü ayar saklama davranışına tabidir; yalnız bellekte tutuldukları iddia
edilmemelidir. Seçilen sağlayıcı çeviri/OCR/ses verisini alır; yerel motorların
gereksinimleri farklıdır. OCR dil dosyaları ve PDFium ilk kullanımda internetten
indirilebilir. Kullanıcı ayarları ve anahtarları Git deposunda tutulmaz.

Native dosya diyaloğu dosyayı yerel yolundan okur. Tarayıcı sürükle-bırak
yüklemesi app cache altında `browser-uploads` içine kopyalar (en çok 2 GB).
Bu yeni upload dizininin otomatik temizliği henüz tamamlanmamıştır. Kullanıcının
kaydettiği çıktılar seçtiği yolda kalır. Eski `/quick` geçici dosyaları kendi
oturum temizliğini kullanır. Native image endpoint'i yalnız config/cache
altındaki izin verilen görselleri sunar.

Native köprü/ayar/olay testleri ve sentetik PDF'nin üç çıktı biçimi ile
çevrilmiş/iki dilli EPUB roundtrip testleri geçmiştir. Gerçek sağlayıcı hesapları,
tam browser etkileşimleri, sistem sesi, OCR ve karmaşık layout çıktılarının
kalan doğrulama listesi devir dosyasındadır. Desenli PDF, görsel kaynağı değiştirme
ve gizlenen özgün metnin seçilebilir kalması gibi eski PDF motoru sınırları
tarayıcıya taşınmayla ortadan kalkmaz.

Geliştirme: `pnpm tauri dev`. Gömülü arayüz kontrolü: `pnpm build` ardından
`cargo build --manifest-path src-tauri/Cargo.toml --features custom-protocol`.
Windows NSIS: `pnpm tauri build --bundles nsis`. Eski installer güncel kaynakla
eşdeğer değildir; son sürüm için yeniden üretilip çalıştırılmalıdır.
