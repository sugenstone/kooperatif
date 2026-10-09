# FUNC-AUDIT-001 — SAHİP ÖZETİ (Basit Türkçe)

Denetim `a0fa441` sürümünde yapıldı. Kod okundu, izole bir veritabanında
gerçek API çağrıları çalıştırıldı, tüm ekranlar ve uç noktalar tek tek
eşleştirildi. **Hiçbir şey değiştirilmedi.**

## Kısa cevap

Program çalışıyor ve var olan her işlem ekrandan yapılabiliyor. Ama
rapor ettiğiniz eksik **gerçek**: sistem sadece TL biliyor. Dolar, Euro
ve altın kasası yaratmak mümkün değil — bunlar hata değil, henüz
yapılmamış ve kararınızı bekleyen özellikler.

## Sorduğunuz konu: Hesap türü ve para birimi

- **Hesap türü seçilebiliyor** ama sadece iki seçenek var: "Kasa" ve
  "Banka". "Altın kasası" diye bir tür yok.
- **Para birimi hiç seçilemiyor.** Hesap açma ekranında da, API'de de
  para birimi alanı yok; veritabanı kural gereği her hesabı TL yapar.
- "İstanbul Doları" adında bir hesap açarsanız sistem bunu kabul eder
  ama içi TL'dir — isim etiket, hesap makinesi TL çalışır. Gerçek dolar
  muhasebesi yok.
- Altın: gram, adet veya saflık diye bir kavram sistemde yok. "Altın"
  yazan kasa da TL sayar.
- Testle doğrulandı: `accountType: gold` göndermek 400 hatası verir;
  `currency: USD` göndermek sessizce yutulup TL kaydeder (bu sessiz
  yutma küçük bir kusur, düzeltilmeli).
- İstanbul/Köy ayrımı diye bir alan da yok — bugün bunu isme yazmak
  gerekiyor ("İstanbul TL Kasası" gibi).

**Sonuç:** Çok para birimli ve altın hesabı onaylı bir ihtiyaç ama
kasdi olarak ertelenmişti. Yapmadan önce şu kararlar lazım: altın gram
mı adet mi, saflık ne, dövizle tahsilat olacak mı, kur nasıl girilecek,
dönüşüm yapılacak mı, raporlar TL'ye mi çevrilecek.

## Yapılabilenler (hepsi doğrulandı)

- Hissedar açma/düzenleme/pasifleştirme, isim çakışması uyarısı, vasi
  atama, aileye bağlama ve aile değiştirme (geçmiş korunur)
- Aile açma, üye listesi, aile tahsilat görünümü
- Hisse açma, devretme, satma (devir ≠ satış), askıya alma/geçersiz
  kılma, sahiplik geçmişi
- Dönem açma, aidat tahakkuku (hissedar başına veya hisse başına),
  önizleme, dönem kapatma
- Tahsilat: ödeyen ≠ borçlu, kısmi/tam/fazla ödeme, tahsis, ters kayıt,
  fazla para → hissedar kredisi → sonraki borca mahsup
- Nakit/banka hesapları, hareket geçmişi, hesaplar arası transfer ve
  ters kayıt, eksi bakiyeye izin yok
- Gelir/gider + kategoriler + ters kayıt (aidat tahsilatı gelir
  sayılmıyor — ayrı sınıf)
- Hisse iadesi → anapara/kâr hakkı → ödeme (kâr belli değilse boş kalır)
- Yatırım: fonlama, gelir, değerleme (nakit yaratmaz), satış, ters kayıt
- Sosyal yardım: fon, bağış (üye olmayan bağışçı serbest isimle),
  yardım ödemesi, ters kayıt, ayrılmış para koruması (kilitli)
- Yönetim: kurul, üyelik, karar, oy, sonuç kaydı (para hareketi yok)
- 15 rapor + genel bakış panosu + canlı TV ekranı
- Kullanıcı pasifleştirme → oturumlar anında düşer; roller ve izinler
  tamamen ekrandan yönetiliyor
- Yedekleme/geri yükleme araçları var ve geri yükleme test edilmiş

## Yapılamayanlar (önem sırasıyla)

1. **Dolar/Euro/altın hesabı** — hiç yok (karar bekliyor)
2. **Arayüzden kullanıcı açma** — yeni operatör ancak komut satırından
   ekleniyor; şifre değiştirme/sıfırlama ekranı da yok
3. **Yanlış aidat tahakkukunu düzeltme** — dönem yanlış kuralla
   üretilirse borç satırları silinemez/düzeltilemez
4. **Üye olmayan kişinin adını düzeltme** (vasi, ödeyen) — kişi düzenleme
   ekranı/uç noktası yok; ancak hissedar üzerinden düzeltilebilir
5. **Aile numarasını düzeltme** — yok
6. **İşlem geçmişi/denetim izini ekrandan görme** — sistem her şeyi
   yazıyor ama okuma ekranı yok
7. **Açılış bakiyesi** — hesap hep sıfırdan başlar
8. **Toplu tahsilat oturumu** (Collection Session) — ertelenmişti, yok
9. **Makbuz/PDF/Excel çıktısı** — yok
10. **Vasi değişiklik geçmişi** — eski vasi kaydı tutulmuyor
11. Küçükler: hesap mutabakat ekranı yok, kişiye not/telefon alanı yok,
    README dosyası eski kalmış, sosyal yardım ekranında tarayıcı
    onay kutusu kullanılıyor (diğer sayfalarda modern dialog var)

## Sizden beklenen kararlar

- Altın: gram mı, adet mi? Saflık (24/22 ayar) takip edilecek mi?
- Dövizle tahsilat/bağış kabul edilecek mi?
- TL↔döviz dönüşümü sistemde yapılacak mı? Kur kim girecek?
- Raporlar tek TL toplamı mı gösterecek?
- Altın alım/satımı transfer mi, dönüşüm mü, yatırım mı sayılacak?
- Sosyal yardım parası döviz/altın olabilir mi?
- İstanbul/Köy ayrımı yapısal alan mı olsun, isim geleneği mi kalsın?
- Mevcut eldeki paralar açılış bakiyesi olarak nasıl girilecek?
- Ayrıca: çıkış haklarında değer koruma (enflasyon/altın/sabit) kuralı,
  yatırım gideri ve kâr-zarar formülleri, karar yeter sayısı kuralları.

## Önerilen sıra

Önce ucuz ve acil olanlar: kullanıcı yönetimi ekranı, tahakkuk
düzeltme, kişi/aile düzeltme, sessiz alan yutma düzeltmesi, denetim izi
görüntüleme. Sonra para birimi/altın temeli (kararlarınızdan sonra en
büyük iş), ardından toplu tahsilat, belge/makbuz sistemi ve üretim
kurulumu.

## Genel değerlendirme

**Denetim tamamlandı — eksikler tespit edildi.** Var olanlar sağlam;
eksiklerin çoğu ya kasdi erteleme ya da karar bekleyen politika. Kritik
yeni gereksinim (çok para birimi + altın) onaylanmış ihtiyaç olarak
belgelerde vardı ama açık kararlar olmadan bilerek yapılmadı.
