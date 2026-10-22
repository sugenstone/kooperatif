# M1-P4 — ÖZEL GÜVENLİK TABLOLARI: MİMARİ VE TEHDİT-MODELİ DENETİMİ

Kapsam: `cooperative_memberships`, `cooperative_number_counters`,
`roles`, `user_role_assignments`, `security_events`. Bu adımda şema,
migrasyon, grant, çalışma kodu veya test değişikliği **yapılmadı**;
veritabanı migrasyonu yok. Denetim `kooperatif_test` (v45) ve kaynak
kod üzerinden yürütüldü.

## 1. Mühür zinciri — doğrulanmış bağımlılık grafiği

```
HTTP isteği
  └─ tenant extractor / gate            ── pool: ÇIPLAK kooperatif_app
  │    ├─ resolve_membership(user,coop)      (cooperative_memberships OKU)
  │    ├─ has_enabled_business_membership    (cooperative_memberships OKU)
  │    └─ list_user_cooperatives             (cooperative_memberships OKU)
  ├─ authz::require_business /
  │    require_in_cooperative           ── pool: ÇIPLAK kooperatif_app
  │    ├─ effective_permissions_*            (ura + roles + memberships OKU)
  │    └─ active_role_summaries_*            (ura + roles + memberships OKU)
  └─ ctx.open_tx → request_tx           ── tx: kooperatif_app + GUC mühür
       ├─ app_open_request_context(hash)  DEFINER/owner:
       │     user_sessions+users doğrula; app.user_id/session_id/
       │     ctx_txid/user_seal damgala (coop=NULL mühürü)
       ├─ app_stamp_cooperative(coop)      DEFINER/owner:
       │     user_seal + txid yeniden hesapla → app_verify_membership
       │     (DEFINER → cooperative_memberships'i OWNER olarak okur,
       │      RLS'ten bağımsız) → app.cooperative_id + ctx_seal damgala
       └─ iş sorguları: politika =
            cooperative_id = app.cooperative_id GUC AND app_ctx_ok()
```

Döngüsel-RLS analizi: mühür zinciri `cooperative_memberships`'i **asla**
uygulama rolüyle okumaz — `app_verify_membership` SECURITY DEFINER
(`kooperatif` sahibi) olduğundan satır güvenliği devreye girmez ve
EXECUTE hakkı `kooperatif_app`'e **verilmemiştir** (P3C testi:
`has_function_privilege(...)=false`). Yani tablo RLS'lense bile mühür
zinciri kırılmaz; kırılan yer **zincirin dışındaki** mühürsüz
okuyuculardır.

## 2. Kritik bulgu — mühürsüz okuma/yazma kulvarları

Standart politika (`cooperative_id = GUC AND app_ctx_ok()`) beş tabloda
aynı gerekçeyle uygulanamaz:

| Yol | Kulvar | RLS altında davranış |
|---|---|---|
| `require_business`/`require_in_cooperative` → `effective_permissions_*` | çıplak pool, GUC yok | 0 satır → **tüm iş istekleri reddedilir (tam kilitlenme)** |
| `resolve_membership` (extractor/gate/switch) | çıplak pool | 0 satır → context kurulamaz |
| `list_user_cooperatives` (`/me`, koop listesi) | çıplak pool | 0 satır → listeleme boş döner |
| `security_events` INSERT (`audit::record*` → `execute(pool)`) | çıplak pool | INSERT reddedilir → **audit kanıtı sessizce kaybolur** (hata yutulur, yalnız log) |
| rbac NULL-kulvarı yazımı (`scoped_visibility` → NULL-attributed) | sealed tx ama `cooperative_id=NULL` | WITH CHECK reddeder → bootstrap/legacy rol yazımı kırılır |

`app_open_request_context` öncesi çalışan hiçbir sorgu GUC taşımaz;
GUC'ler tx-local olduğundan pool seviyesindeki okumalarda mühür
matematiksel olarak mevcut olamaz. Bu, "ENABLE+FORCE her tabloda doğru
çözümdür" varsayımını bu beş tablo için geçersiz kılar.

## 3. Belge numaralandırma — `cooperative_number_counters`

- Grant'ler: `kooperatif_app`'e **hiçbir hak yok** (0035 daraltması).
- Tüm erişim SECURITY DEFINER allocator'larda (`allocate_document_number`,
  `reserve_document_number`; owner=`kooperatif`, P3C sealed-context
  kontrollü, EXECUTE mühürlü çağırana açık).
- Doğrudan SQL tehdit modeli: `kooperatif_app` ile `SELECT/UPDATE`
  → grant yokluğunda zaten permission-denied. RLS **ek bir görünürlük
  katmanı ekler** (gelecekteki yanlış GRANT'ı bile etkisiz kılar) ama
  mevcut durumda korunan yeni bir yüzey yok.
- Karar: ENABLE+FORCE + yalnız `owner_lane`. Uygulama politikası
  **gerekmez** — politika yokluğu app rolü için zaten tam ret demektir
  (fail-closed). En düşük riskli, sıfır iş-akışı etkili adım.

## 4. Nullable system-lane üçlüsü

| Tablo | NULL satır anlamı | Coop satır anlamı | App grant'leri |
|---|---|---|---|
| `roles` | legacy/system-attributed rol | coop'a ait rol | SELECT,INSERT,UPDATE |
| `user_role_assignments` | NULL-attributed atama | scoped atama | SELECT,INSERT,DELETE |
| `security_events` | platform olayı (login, user lifecycle) | coop-attributed olay | INSERT,SELECT |

NULL-kulvarı görünürlüğü yalnız **doğrulanmış bootstrap scope**'unda
meşru (`scoped_visibility` ve `effective_permissions_in_bootstrap`
kodlanmış kural: NULL yalnız marker+is_bootstrap tutarlılığında
bootstrap coop'a atfedilir, başka scope'ta asla). Bu kural RLS'te
yeniden ifade edilmelidir — aksi halde bootstrap kulvarındaki legacy
NULL satırlar görünmez olur ve eski kurulumların izinleri kaybolur.

## 5. Tablo bazında güvenlik karar matrisi

| Tablo | Doğrudan app erişimi | Definer bağımlılığı | Kalan saldırı yolu | Öneri | Karar |
|---|---|---|---|---|---|
| `cooperative_number_counters` | yok | allocator'lar | yalnız gelecek-GRANT hatası | ENABLE+FORCE + owner_lane | **GO — tek başına** |
| `security_events` | INSERT+SELECT çıplak | yok | sahte/NULL olay enjeksiyonu (bugün de var), cross-coop okuma | önce kod: `app_record_security_event` definer'i (damgalı user/coop doğrular); sonra ENABLE+FORCE + SELECT-scoped, app INSERT yok | **KOD REFAKTÖRÜ SONRASI GO** |
| `cooperative_memberships` | SELECT çıplak (extractor/gate/authz/me) | `app_verify_membership` zincir içi | cross-coop roster sızdırma | önce kod: authz/extractor okumalarını definer'e taşı (`app_user_permissions`, `app_my_cooperatives`); sonra ENABLE+FORCE + owner_lane (app SELECT sıfır) | **KOD REFAKTÖRÜ SONRASI GO** |
| `roles` | SELECT/INSERT/UPDATE (authz çıplak + sealed CRUD) | yok | cross-coop rol görünürlüğü, NULL-lane sızması | authz definer'i sonrası ENABLE+FORCE; app politikası: coop-scoped + NULL-yalnız-bootstrap | **KOD REFAKTÖRÜ SONRASI GO** |
| `user_role_assignments` | SELECT/INSERT/DELETE (authz çıplak + sealed yönetim) | yok | cross-coop atama görünürlüğü | `roles` ile aynı tasarım | **KOD REFAKTÖRÜ SONRASI GO** |

## 6. Güvenli migrasyon sırası (öneri)

1. **Adım S1 (yalnız migrasyon, sıfır risk):** `cooperative_number_counters`
   ENABLE+FORCE + `owner_lane`. Sınır assertion'ları 38 tablo / 75
   politikaya güncellenir (bu tabloda app politikası yok — 2/table
   varsayımı bu tablo için 1'e düşer, testler buna göre yazılır).
2. **Adım S2 (kod + migrasyon):** definer katmanı —
   `app_record_security_event(type, coop|null, meta)` (damgalı user ile
   bağlı; NULL yalnız platform olayları), `app_effective_permissions`,
   `app_my_cooperatives`, `app_resolve_membership` (user-seal bağlı,
   minimal kolonlar). Çağrı yerleri definer'e çevrilir; `record_scoped`
   pool yerine definer'i kullanır. **Audit yazımı artık fail-loud
   yapılabilir** (bugün sessiz yutma tasarımı korunur ama ret
   görünür hale gelir).
3. **Adım S3 (migrasyon):** `security_events` ENABLE+FORCE —
   SELECT `cooperative_id = GUC AND app_ctx_ok()`; app INSERT yok
   (definer yazıyor); NULL satırlar app'e görünmez (platform olayları
   owner kulvarında kalır — `never_bootstrapped` sorgusu owner pool'da,
   etkilenmez).
4. **Adım S4 (migrasyon):** `cooperative_memberships` ENABLE+FORCE +
   owner_lane — uygulamanın tüm okumaları artık definer'da.
5. **Adım S5 (migrasyon):** `roles` + `user_role_assignments`
   ENABLE+FORCE — app politikası `cooperative_id = GUC AND
   app_ctx_ok()` **artı** NULL-lane yalnız `app_ctx_bootstrap()`
   definer'iyle (sealed coop marker+is_bootstrap tutarlıysa NULL
   satırlar görünür). CRUD sealed tx'te çalışmaya devam eder.

## 7. Stop/Go kriterleri

- **GO (S1):** `cooperative_number_counters` — bağımsız, kanıtlanmış
  sıfır app grant'i; tam suit + odaklı testler yeşilse ilerler.
- **STOP (S2-S5):** definer refaktörü olmadan `cooperative_memberships`,
  `roles`, `user_role_assignments`, `security_events`'e RLS uygulamak
  **tam kilitlenme ve sessiz audit kaybı** üretir — kanıtlandı.
- Her adım için: odaklı + tam suit (RUST_TEST_THREADS=6), fmt/clippy,
  taze disposable replay, `kooperatif_dev` dokunulmazlığı.

## 8. Kalıcı riskler

- S2 refaktörüne kadar `kooperatif_app` çıplak-pool okumalarıyla
  membership/rol roster'ları cross-coop görünür (bugünkü grant gerçeği;
  kullanıcıya dönük SQL yüzeyi yok — teorik SQL-injection kulvarı).
- `security_events` INSERT'leri mühürsüz — sahte coop atıflı olay
  yazımı grant seviyesinde mümkün; definer'e taşınana dek sürer.
- `never_bootstrapped`'ın `security_events` koşulu owner pool'da
  koştuğundan RLS sonrası da doğru kalır.
- `cooperative_memberships`/`roles`/`ura`/`security_events` için
  standart iki-politika deseni **bozulacak** (definer-arka kulvarlar +
  NULL-lane bootstrap koşulu) — sınır assertion'ları tablo bazında
  yeniden yazılmalıdır.
