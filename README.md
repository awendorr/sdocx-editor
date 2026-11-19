# sdocx-editor

Samsung Notes (`.sdocx`) notlarını Samsung Notes olmadan bilgisayarda açıp görüntülemek, düzenlemek ve tekrar `.sdocx` olarak kaydetmek için Rust ile yazılmış bir masaüstü uygulaması. Samsung ile bir bağlantısı yoktur.

*A Rust desktop app to view, edit and save Samsung Notes `.sdocx` files without Samsung Notes. Not affiliated with Samsung.*

## Neler yapabiliyor

- **Açma ve görüntüleme:** el yazısı (kalem türleri dahil), yazı kutuları, resimler, not başlığı ve klavyeyle yazılan not metni. Sayfalar [`sdocx`](https://github.com/twangodev/sdocx) kütüphanesinin çizicisiyle aslına yakın gösterilir.
- **Düzenleme:** kalem, vurgulayıcı, silgi (dokunduğu çizgiyi siler), seçip taşıma, silme, yazı kutusu ekleme/düzenleme, resim ekleme ve boyutlandırma, sayfa ekleme/silme, başlık ve not metnini düzenleme, geri al/yinele.
- **Kaydetme:** `.sdocx` olarak. Dokunulmayan sayfalar, nesneler ve medya dosyaları kaynak dosyadan **bayt bayt aynen** kopyalanır; yalnızca değişen kısımlar yeniden yazılır. Her kayıttan önce dosya bağımsız okuyucuyla yeniden açılır ve Samsung'un SHA-256 bütünlük özetleri doğrulanır.
- **Güvenli kayıt:** açılan bir notu ilk kez kaydederken varsayılan ad `… (düzenlendi).sdocx` olur. Aynı dosyanın üzerine yazarsanız önce `.sdocx.yedek` kopyası alınır.
- **PDF olarak dışa aktarma** (vektör).
- **Önizleme modu:** notu tam olarak kaydedileceği hâliyle gösterir.
- **Sade, modern arayüz:** ortalanmış araç çubuğu; kaleme, vurgulayıcıya ve silgiye ikinci kez tıklayınca açılan kalınlık, saydamlık ve 30 renklik palet; son kullanılan renkler; sayfa üzerinde doğrudan yazı yazma; seçili öğenin üstünde renk/boyut/silme çubuğu; sayfa küçük resimleri; açık ve koyu tema. Resimleri pencereye sürükleyip bırakarak da ekleyebilirsiniz.

## Sınırlamalar

- **Kilitli (şifreli) notlar açılamaz.** Bunu çözen bilinen bir proje yok. Notun kilidini telefonda kaldırıp yeniden dışa aktarın.
- **Yeni çizimler tek kalem türüyle kaydedilir** (dolma kalem). Basınç algılanmaz. Var olan bir çizimi taşırsanız veya rengini değiştirirseniz o çizim de dolma kaleme dönüşür.
- **Şekil, tablo, formül, ses kaydı, bağlantı, döndürülmüş/kırpılmış resim veya yazı kutusu** gibi öğeler görünür ve korunur ama düzenlenemez (silinebilir).
- **Not metni değiştirilirse biçimlendirmesi** (kalın, renk, liste, başlık stili) sadeleşir. Değiştirmezseniz olduğu gibi kalır.
- **PDF arka planlı sayfalarda** PDF ekranda görünmeyebilir. Dosyada korunur.
- Birden fazla katmanı olan sayfalarda yalnızca etkin katman düzenlenir, diğerleri aynen korunur.
- Kaydedilen dosyalar, formatın tersine mühendislik belgelerine ve bütünlük kontrollerine göre doğrulanmıştır ama **Samsung Notes uygulamasının kendisinde test edilmemiştir**. Önemli notlarınızın yedeğini saklayın.

## Kurulum

**Windows:** [Releases](../../releases) veya Actions sayfasından `sdocx-editor.exe` dosyasını indirip çalıştırın. Bir `.sdocx` dosyasını pencereye sürükleyebilir ya da "Birlikte aç" ile açabilirsiniz.

**Kaynaktan derleme** (Rust 1.92 veya üstü):

```sh
cargo build --release -p sdocx-editor
./target/release/sdocx-editor not.sdocx
```

Linux'ta `libxkbcommon-x11` ve OpenGL gerekir.

## Kısayollar

| Kısayol | İşlev |
| --- | --- |
| Ctrl+O / Ctrl+S / Ctrl+Shift+S | Aç / Kaydet / Farklı kaydet |
| Ctrl+Z / Ctrl+Y | Geri al / Yinele |
| V, P, H, E, T | Seç, Kalem, Vurgu, Silgi, Yazı |
| Del | Seçili öğeyi sil |
| Ctrl + fare tekerleği, Ctrl + / Ctrl − | Yakınlaştır / uzaklaştır |
| Ctrl+0 | Sayfa genişliğine sığdır |
| Esc | Yazmayı bitir / seçimi kaldır |
| Seç aracında çift tıklama | Yazı kutusunu düzenle |

## Proje yapısı

- `crates/sdocx-edit`: düzenlenebilir model, içe aktarma, `.sdocx` yazıcı, doğrulama, sayfa çizimi ve PDF.
- `crates/sdocx-editor`: egui/eframe masaüstü arayüzü.

Testler (`cargo test -p sdocx-edit`) örnek notları açar, değiştirir, kaydeder ve kaydedilen dosyayı `sdocx` okuyucusuyla bütünlük kontrolü açık olarak yeniden okur.

## Teşekkürler ve lisans

- Okuma ve çizim: [twangodev/sdocx](https://github.com/twangodev/sdocx) (GPL-3.0).
- Yazma: [Downhillghost/Dietrich](https://github.com/Downhillghost/Dietrich) (MIT) projesindeki `.sdocx` yazıcısının Rust uyarlaması (`crates/sdocx-edit/src/encode.rs`, `templates.rs`).
- Format araştırması: [squ1dd13/sdocx2pdf](https://github.com/squ1dd13/sdocx2pdf), [DomHeadroom/sdocx-hexpatterns](https://github.com/DomHeadroom/sdocx-hexpatterns), [0xA01F3/sdocx_parser](https://github.com/0xA01F3/sdocx_parser) (test örnek dosyaları bu projeden, GPL-3.0), [bxff/samsung-notes-format](https://github.com/bxff/samsung-notes-format) (MIT).

`sdocx` GPL-3.0 olduğu için bu proje de **GPL-3.0-or-later** lisanslıdır. Ayrıntılar için [LICENSE](LICENSE).
