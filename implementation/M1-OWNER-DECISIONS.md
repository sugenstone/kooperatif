# M1 — SAHİP KARAR PAKETİ (OWNER DECISIONS)

PROJECT-CONTROL-002 / M1-ARCH-001. **Bu belge yalnızca planlamadır; hiçbir
uygulama kodu veya migration yazılmamıştır.** PD-01 (ortak PostgreSQL,
katı izolasyon, RLS yalnızca derinlemesine savunma) ve PD-02
(kullanıcı birden çok kooperatifte bağımsız roller) **onaylıdır —
yeniden onay istenmemektedir.**

Mevcut zayıflık özeti: 43 iş tablosunun **hiçbirinde** tenant sütunu
yok; roller/izinler global; oturumda kooperatif yok; `pg_notify`
sadece domain etiketi taşıyor; 20 tablodaki iş numaraları (makbuz
dahil) **global IDENTITY** ile üretiliyor; hiçbir RLS politikası yok;
`/api/auth/me` global izin döndürüyor. Foreign key'ler sadece
"varlık" denetliyor — bugün tek kooperatif olduğu için zararsız,
çok kooperatifte IDOR (ID tahminiyle çapraz kayıt bağlama) riski doğurur.

---

## M1-K1 — Aktif kooperatif bağlamı (context) mekanizması

**Öneri: Hibrit (E)** — her istekte `X-Cooperative-Id` başlığı;
oturumda `active_cooperative_id` yalnızca UX varsayılanı.

| Seçenek | Artı | Eksi |
| --- | --- | --- |
| A. URL `/api/coops/{id}/…` | Sekme başına doğal izolasyon | 82 route yeniden adlandırma; ID log/bookmark'ta; çirkin API |
| B. Oturumda tek aktif koop | Basit | **İki sekme aynı anda farklı koop gösteremez** (D03 zorunluluğunu bozar); sekme değişince sessiz bağlam kayması |
| C. Yalnız header | Temiz, CSRF-dirençli | Sekme-ötesi UX varsayılanı yok |
| D. İmzalı context token | Güçlü ispat | Gereksiz kripto karmaşası |
| **E. Header + oturum varsayılanı** | **Sekmeler bağımsız**, backend her istekte üyeliği doğrular, CSRF direnci, düşük maliyet | Client disiplini gerekir (`apiFetch` tek noktadan ekler) |

**Sonuç:** Kullanıcı aynı anda iki sekmede A ve B kooperatifini ayrı
ayrı görüntüleyebilir; yetki sızıntısı olmaz çünkü her istek backend'de
üyeliğe karşı doğrulanır.

## M1-K2 — `persons` (gerçek kişiler) global mi, kooperatife mi ait?

**Öneri: Kooperatife ait (A).**

| | A. Koop-owned | B. Global |
| --- | --- | --- |
| PII izolasyonu | **Tam** — B kooperatifi A'nın kişi kaydını asla göremez | Zayıf — kişi tablosu tüm kooplara görünür olur |
| Tutarlılık | Her FK same-coop — mevcut modelle uyumlu | Her person sorgusuna ikincil kural gerekir |
| Aynı insan 2 koopta | 2 ayrı satır (kabul edilebilir; sonradan "link" özelliği eklenebilir) | Tek satır — ama geri dönüşü yok |
| Geri döndürülebilirlik | Link sonradan eklenebilir | Sızmış PII'yi geri izole etmek neredeyse imkânsız |

**Sonuç:** A kooperatifindeki "Ahmet Yılmaz" ile B'deki aynı insan iki
ayrı kayıt olur. Mahremiyet kazanımı > veri tekrarı maliyeti.

## M1-K3 — `financial_categories` ve varsayılan rol seti

**Öneri: Kooperatife ait + şablon kopyalama.** Bugün
`financial_categories` migration ile seed'leniyor; kooperatif
oluşturma sırasında kategori + rol şablonu yeni koopa klonlanır
(mevcut seed deseni korunur). Her koop kendi kategorisini/rolünü
özelleştirebilir; diğerini etkilemez.

## M1-K4 — İş numarası üretimi (makbuz/dönem/hisse/ödeme no'ları)

**Öneri: `(cooperative_id, kind)` sayaç tablosu — koop başına ardışık.**

Bugün 20 tablo `GENERATED ALWAYS AS IDENTITY` ile **global** sıra
üretiyor. Registry REQ-024.6 "per-coop numbering" ister.

| | Global identity devam | Koop-sayaç tablosu |
| --- | --- | --- |
| A koopunda makbuz no'ları | 1, 5, 9, 23… (boşluklu — diğer koop araya girer) | 1, 2, 3… ardışık |
| Türkiye pratiği (ardışık makbuz beklentisi) | Sorunlu | Uygun |
| Karmaşıklık | Sıfır | Bir tablo + FOR UPDATE satır kilidi (mevcut ödeme disipliniyle aynı) |
| Performans | En iyi | Koop başına tür başına tek satır kilit — koop ölçeğinde ihmal edilebilir |

**Soru:** Kapsam 21 numara türünün tümü mü, yoksa yalnızca
makbuz/ödeme no'ları mı? Önerim **tümü** — yarım ölçüde global numara
kullanıcı kafa karışıklığı yaratır.

## M1-K5 — Roller: koop başına kopya mı, global şablon mu?

**Öneri: Kooperatife ait roller + oluşturma anında şablon klonu.**

Global rol kataloğu seçeneğinde tek "Sayman" tanımı tüm kooplarda aynı
izinleri taşır; şablonda yapılan bir izin değişikliği **tüm
kooperatifleri sessizce etkiler** — finansal yetkiler için tehlikeli.
Koop-owned roller PD-02'nin "koop başına bağımsız" ruhuna birebir uyar.

## M1-K6 — RLS katmanı + `kooperatif_app` non-superuser rolü

**Öneri: Kabul — M1-P4'te uygulanır.**

| | A. Yalnız uygulama scoping + composite FK | B. + RLS |
| --- | --- | --- |
| Unutulan `WHERE cooperative_id` | **Sızar** (test/review'a bağlı) | **Fail-closed** — yanlış bağlam boş sonuç döndürür |
| Maliyet | Düşük | `kooperatif_app` ayrı rolü (compose+CI+`.env`), her sorgu `SET LOCAL` içeren tx'te, `after_release` reset |
| Operasyon | Değişmez | Migration'lar sahip rolüyle, uygulama `kooperatif_app` ile — dokümante |

Dev konteynerinde `kooperatif` superuser → RLS sessizce bypass olur;
RLS'in anlamlı/test edilebilir olması için ikinci rol **şarttır**.
Sonuç: kabul ederseniz docker-compose/CI'a ikinci DB rolü eklenir
(sadece dev/test altyapısı; VDS'e dokunulmaz).

## M1-K7 — Platform yönetimi (kim kooperatif oluşturur?)

**Öneri: M1 için yalnız CLI** — `kooperatif-server create-cooperative` /
`add-member` (mevcut `create-user`/`grant-role` deseni). HTTP yüzeyi
sıfır; ileride ihtiyaç çıkarsa dar kapsamlı `platform_admin` ayrı
değerlendirilir. **Global super-admin data bypass'ı tasarlanmamıştır.**

---

## Karar özeti tablosu

| # | Konu | Öneri | Onay gerekli |
| --- | --- | --- | --- |
| M1-K1 | Context: header + oturum varsayılanı | Hibrit E | ✅ |
| M1-K2 | persons | koop-owned | ✅ |
| M1-K3 | kategoriler+rol seed | şablon klonu | ✅ |
| M1-K4 | numaralandırma | tümü koop-sayaç | ✅ (kapsam onayı) |
| M1-K5 | roller | koop-owned | ✅ |
| M1-K6 | RLS + app rolü | kabul | ✅ |
| M1-K7 | coop oluşturma | CLI-only | ✅ |

Tümü önerilen şekliyle onaylanırsa faz sırası:
**P0 tenant kernel → P1 RBAC → P2 (a–j) iş tabloları → P3 numaralandırma
→ P4 RLS → P5 realtime+audit → P6 admin API → P7 frontend → P8 test
matrisi → P9 koop-bazlı export.** UI ancak backend izolasyonu kanıtlandıktan
sonra açılır.
