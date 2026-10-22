# M1-P4 — BAĞIMSIZ FİNAL KAPANIŞ AUDIT'İ (READ-ONLY)

Kapsam: migration 0032–0057, remediasyon R1, R2, R3, R3B, R3C, R4.
Yöntem: canlı `kooperatif_app` bağlantısı (gerçek credential),
`pg_catalog`/`information_schema` sorguları, migration kaynağı
incelemesi, gerçek sunucu E2E (restricted principal). Önceki ajan
raporları kanıt olarak kabul edilmedi; tüm sınıflandırmalar yeniden
üretildi.

Doğrulama ortamı: PostgreSQL 17.11, `kooperatif` superuser (owner),
`kooperatif_app` (LOGIN, NOSUPERUSER, NOBYPASSRLS), disposable
`kooperatif_test_*` DB'leri. `kooperatif_dev` (v33) ve tarihsel
`kooperatif` (v20) DB'lerine **dokunulmadı**.

---

## 1. Bulgu kanıt matrisi (F1–F7)

| Bulgu | Orijinal zaaf | Remediasyon | Canlı kanıt | Kalan bypass | Karar |
|---|---|---|---|---|---|
| F1 | `role_permissions` DB sınırı yok — app SELECT+graft | 0052 + 0057: SELECT revoke, yazımlar definer | `kooperatif_app` `SELECT role_permissions` → `permission denied` | Yok | **PASS** |
| F2 | `ura`/`roles` doğrudan yazım | SELECT-only grant; definer yazımları | grant matrisi: iki tabloda yalnız SELECT; `ura_coop_role_fk`/`coop_user_fk` kompozit FK'lar owner-kulvarda bile üyelik/rol eşleşmesi dayatıyor (canlı ret) | Yok | **PASS** |
| F3 | `users` kapsamsız UPDATE/SELECT | 0053: SELECT 4 kolona dar (`id,username,display_name,status`), status yazımı `app_user_set_status` definer'ı | column_privileges canlı listesi; UPDATE hakkı yok | Yok | **PASS** |
| F4 | Finansal tarihte DELETE/UPDATE | 0054: DELETE yalnız `periods`+`assessment_rules` (draft), UPDATE 8 tabloda kolon-bazlı | canlı grant listesi: `payments`/`account_movements`/`income_entries`/... UPDATE-DELETE yok; `shares` yalnız `updated_at` | Oluşturma-yolu eksik-paket (bkz. B1) | **PASS (B1 rezidüeli)** |
| F5 | Serbest `app_ctx_audit` event_type + audit'siz raw-SQL yazımı | 0057: whitelist (`shareholder_family_changed`, `assessments_generated`), `security_events` sıfır grant, 35 tabloda transition-audit trigger'ı | mühürlü tx'te `app_ctx_audit('payment_reversed')` → 42501; `payment_reversed` tipi artık trigger-mint; sahte mühür → `sealed request context required` | Guard-only tablolarda audit'siz yazım (B2) | **PASS (B2 rezidüeli)** |
| F6 | `cooperatives`/`installation_state`/`permissions`/`role_permissions` enumeration | 0057: SELECT revoke + 11 definer reader | mühürlü + mühürsüz `kooperatif_app`'te dört tablo `permission denied`; definer'lar EXECUTE'lu ve dar sonuç | Yok | **PASS** |
| F7 | `account_movements.source_*` FK'siz uydurma atıf | 0057: `app_movement_source_pre` (atıf pin'i) + deferred `app_movement_source_check` (aynı-coop/domain/backlink/yön/tutar/hesap) | R4 adversarial testi cross-coop/invalid ret kanıtlı; canlıda consistent-ref commit (B1 kapsamı) | Consistent-fabrication B1'de | **PASS (B1 rezidüeli)** |

---

## 2. Yeni bulgular (R4 derinlemesine audit sonucu)

### B1 — MEDIUM: Yetkili-kulvar eksik-paket üretimi (creation-path)

**Exploit (canlı üretildi, `kooperatif_test_fff…` disposable DB):**

```sql
-- kooperatif_app, gerçek sealed ctx (app_open_request_context +
-- app_stamp_cooperative, Sistem Yöneticisi izinleri)
INSERT INTO persons (...) VALUES (...);            -- write_guard geçti
INSERT INTO payments (..., status='posted',
    destination_account_id=..., amount=5000) ...;  -- write_guard geçti
INSERT INTO account_movements (..., source_type='payment',
    source_id=<payment>, direction='inflow', amount=5000) ...;
COMMIT;  -- BAŞARILI
```

Sonuç: `payments.status='posted'` + `account_movements` commit'lendi
→ türetilmiş bakiye +5000; `payment_allocations` = 0; tahakkuk
mahsubu / kredi uygulaması / belge-numaralandırma akışı atlandı.
Audit yine mint edildi (`payment_posted`, atıf = sealed aktör) —
ama fabrike edilmiş business fact'i meşru gibi kaydetti.

**Sınırlar:** gerçek mühürlü oturum + ilgili `*.manage` izni + app
credential'a SQL erişimi gerekir; tek coop; aktör atfı sahtelenemez;
cross-coop imkânsız; tüm satırlar audit'e düşer. HTTP katmanı doğru
— yalnız app-lane raw SQL / app credential compromise / SQLi
senaryosu.

**Kök neden:** R3C paket-atomikliği yalnız reversal/lifecycle
yazımlarına uygulandı; *oluşturma* yolları (post_payment vb.) tasarım
gereği repo-katmanı çoklu INSERT'idir. `app_write_guard` izni
doğrular, paket-bütünlüğünü doğrulamaz.

**Öneri (uygulanmadı — karar):** finansal-olay oluşturma yollarının da
`_tx` definer'larına taşınması (M1-P5 adayı) veya
`app_movement_source_check`'e "kaynak satırı aynı tx'te paket-tamam"
koşulunun eklenmesi.

### B2 — LOW: Audit'siz guard-only yazım yüzeyi

`app_write_guard`'ı olup transition-audit trigger'ı olmayan 6 tablo:
`account_movements`, `assessments`, `assessment_rules`,
`assessment_share_sources`, `investment_disposal_proceeds`,
`share_ownerships`. Yetkili doğrudan INSERT'larda özel audit event'i
mint edilmez (`account_movements` için kaynak-event'i üzerinden örtü
var; diğerleri için route-katmanı `assessments_generated` /
`investment_disposed`/`share_transferred` event'leri kapsar — ama
doğrudan yazım bu event'leri atlar).

### B3 — LOW: `payment_allocations_added` semantik kayması

R4 öncesi route tek aggregate event basıyordu
(`added_allocation_count`, `allocated_amount`,
`unallocated_amount`); trigger artık allocation-satırı başına event
mint ediyor ve aggregate metadata kayboldu. Satır-atfı daha hassas;
kardinalite değişimi rapor tüketicilerini etkileyebilir — bilinçli
kabul veya metadata zenginleştirme kararı gerekir.

### B4 — LOW: Domain HTTP suiti owner-kulvarda

`governance.rs`, `credits.rs` vb. domain test binary'leri Router'ı
`kooperatif` (owner) pool'u ile kuruyor → `app_write_guard`
(`session_user` kontrolü) ve `app_audit_emit` (`ctx_txid NULL →
silent return`) bu akışlarda devreye girmiyor. Kısıtlı-kulvar
kapsamı: `m1p4_rls_pilot` (108 raw-SQL probe), `authz_cooperative`
(DB-seviye), R4 adversarial test ve bu audit'te manuel koşulan gerçek
HTTP E2E (aşağıda). HTTP→Router→repo→DB akışının `kooperatif_app`
üzerinden tam-suite doğrulaması yok — test boşluğu, ürün kusuru değil.

### B5 — BİLGİ: `security_events.user_id` FK'si owner-kulvarda bile
aktör satırını koruyor — probe kullanıcısı silinemezken audit satırı
FK'si engelledi (tasarımla uyumlu, olumlu).

---

## 3. Tenant izolasyonu (bağımsız)

| Kontrol | Sonuç |
|---|---|
| Mühürsüz `kooperatif_app` SELECT (payments/cm/users/roles/ura) | 0 satır (fail-closed) |
| Mühürsüz INSERT (business tablo) | `app_write_guard` → 42501 |
| Sahte GUC + sahte mühür | `app_ctx_audit` → `sealed request context required` |
| `app_seal_of` EXECUTE | `kooperatif_app`'e kapalı — mühür self-mint edilemez |
| Gerçek mühür (sealed) | `app_ctx_ok=t`; whitelist dışı event → 42501 |
| Pool bağlantı-yeniden-kullanımı | `before_acquire`: tüm app.* GUC + ROLE RESET |
| Production owner-kulvarı | `KOOPERATIF_APP_DATABASE_URL` zorunlu; `KOOPERATIF_ALLOW_ADMIN_RUNTIME` production'da reddedilir; `inspect_runtime_principal` boot'ta privileged bağlananı reddeder (dev'de açık opt-in + WARN) |
| Komut-satırı (create-user/add-member/bootstrap/migrate) | `KOOPERATIF_DATABASE_URL` owner-kulvarı + `database_identity_guard` — tasarım gereği privileged, audit'te belgeli |
| İkinci kooperatif izolasyonu | 42 FORCE-RLS tablo + kompozit `*_coop_fk` filosu (owner-kulvarında bile üyelik-öncesi-rol reddi canlı doğrulandı) |

---

## 4. R4 audit-atomikliği incelemesi (24 paket + non-paket)

- **Kapsam:** 35 tabloda `app_audit_transition`; 39 tabloda
  `app_write_guard`; `account_movements`'te ek 2 trigger.
- **24 `_tx` paketi:** EXECUTE grant'lı 24/24; tüm mutasyonları
  trigger'lı tablolarda → paket çağrısı audit'siz tamamlanamaz;
  audit hatası işlemi rollback'ler; rollback'te satır kaybolur
  (R4 testi kanıtlı).
- **Audit-only 3 tablo** (`roles`, `ura`, `users`): doğrudan INSERT
  grant'i yok → guard gerekmiyor; definer yazımları sealed GUC'tan
  mint eder — doğru.
- **Recursion:** `security_events`'te audit trigger'ı yok (yalnız FK
  internal trigger'ları) → sonsuz döngü yok.
- **Atıf:** aktör/coop `app_audit_emit` içinde mühürden doğrulanır;
  `metadata` business fact'ler; decimal'lar canonical string.
- **Duplicate:** route-katmanı `record_tx` yalnız 2 whitelist tipi +
  owner-lane/bootstrap + session telemetry'de kaldı → çift-audit
  testi temiz (count==1 assertion'ları geçiyor).
- **Kalan açıklar:** B1 (oluşturma-paketi bütünlüğü), B2 (audit'siz
  guard-only tablolar), B3 (aggregate→per-row drift).

---

## 5. `users.status` mimarisi — DECISION GATE (uygulanmadı)

Mevcut: `users.status ∈ {active,disabled}` **global** kimlik
özniteliği; herhangi bir coop'un `users.manage`+member_visibility'li
admin'i kullanıcıyı global devre dışı bırakabilir → kullanıcının
diğer coop'larındaki erişimi de kapanır.
`cooperative_memberships.status ∈ {active,suspended,ended}` şemada
mevcut; `app_stamp_cooperative` zaten `membership_status='active'`
şartlıyor — ama runtime'da suspend/unsuspend akışı yok.

**Önerilen ayrım:**
- `users.status` → platform-kimlik durumu: yalnız platform-operatörü
  (owner-kulvar/CLI) veya self-service deaktivasyon; coop admin'i
  yazamaz.
- `cooperative_memberships.status='suspended'` → coop-bazlı
  süspansiyon: `app_member_set_status` definer'ı (sealed ctx +
  `users.manage` + aynı-coop üyelik); `app_stamp_cooperative` zaten
  süzüyor; audit: `cooperative_membership_suspended/resumed`
  trigger-mint; HTTP endpoint + UI durum gösterimi; login global
  kalmalı ama suspend edilmiş coop'ta mühürleme reddedilmeli.
- **Gerekli değişiklikler:** migration (definer + audit event +
  gerekirse `app_user_set_status`'ın yetki modelini platform-lane'e
  kısıtlama), API (member-suspend endpoint'leri), audit event'leri,
  UI (üyelik durumu rozeti), migration-backfill (mevcut 'disabled'
  kullanıcıların coop-bazlı eşlemesi).
- **Açık domain sorusu:** kullanıcıyı tüm kooperatiflerden kapatma
  meşru bir operasyon mu? Platform-rolü var mı? — ürün kararı.

---

## 6. Browser/API hazırlığı

**Frontend envanteri:** `apps/web` (SvelteKit), 53 sayfa / 22 domain
(aileler, donemler, finansal-hesaplar, gelir/gider, hisse-iadeleri,
hissedarlar, hisseler, kategoriler, kullanicilar, oturumlar,
raporlar, roller, sosyal-yardim, tahakkuklar, tahsilatlar,
transferler, yatirimlar, yonetim, gelir-gider + `(tv)/canli-ekran`).
Üyelik yönetimi (member suspend/invite) UI'da görünmüyor —
backend-only/CLI.

**Bu audit'te canlı koşulan restricted-role HTTP E2E:**
`kooperatif-server serve` `KOOPERATIF_APP_DATABASE_URL` ile
`kooperatif_app` principal'ında boot etti (`restricted runtime
principal verified` logu) → `POST /api/auth/login` 200 →
`GET /api/cooperatives` 200 (üyelik listesi) →
`POST /api/auth/cooperative` 204 (mühürlü ctx) →
`GET /api/financial-accounts` 200 + `GET /api/payments` 200 —
tümü `kooperatif_app` kulvarında RLS+guard+seal ile. CSRF reddi
(Origin'siz POST → `csrf_failed`) ayrıca doğrulandı.

**RLS/grant/trigger değişikliğinden bozulan özellik yok** — tam suit
(~690 test) R4 sonrası yeşil; definer reader'lara rewired olan
yüzeyler (permissions, roller, admin-count, bootstrap) testte geçti.

---

## 7. Backup / migration / rollback hazırlığı

| Kontrol | Durum |
|---|---|
| Migration replay 0001→0057 (temiz DB) | PASS — her per-test DB sıfırdan uygular; `_sqlx_migrations` v50-57 `success=t`, checksum'lar mevcut |
| Checksum bütünlüğü | PASS — dosya-checksum uyuşmazlığı sqlx'i fail eder (audit'te gözlendi, manuel düzeltme yok) |
| `kooperatif_dev` / tarihsel `kooperatif` | v33 / v20 — dokunulmadı |
| Backup altyapısı | `backup:create/verify/restore/status/retention/drill/failure-test` + 27/27 unit test; `EXPECTED_DATABASE` kimlik-guard'ı; manifest+sha256 |
| Son gerçek yedek | `2026-10-12` — **0055/0056/0057 öncesi** |
| Son restore doğrulaması | `2026-10-12` PASS (`kooperatif_c53_restore_verify`) — **0057 öncesi** |
| ADR-013 kapısı | `backup:drill` izole akış hazır ama **0057 sonrası koşulmadı** — closure öncesi zorunlu |
| Rollback stratejisi | Migration'lar append-only; geri-alma = forward-fix migration; restore = backup'tan (DR-rollback iş hatası düzeltmek için değil) |
| Runtime principal ayrımı | Production'da `KOOPERATIF_APP_DATABASE_URL` zorunlu + privileged-principal boot reddi |

---

## 8. Karar

**M1-P4 closure: CONDITIONAL GO.**

F1–F7 tamamı bağımsız kanıtla PASS. Tenant izolasyonu, mühür
sahteciliği, enumeration ve audit-atomikliği canlıda doğrulandı.
Yeni bulgular B1 (MEDIUM) ve B2–B4 (LOW) exploitabilitesi sınırlı,
atfı-bozulmamış, tek-coop-sınırlı ve tam-audit'li — closure'ı
engellemiyor ama kayıt altına alınmalı.

**İkinci kooperatif aktivasyonundan önce zorunlu minimum:**

1. Post-0057 `pnpm backup:drill` + restore doğrulaması (ADR-013).
2. `users.status` global-vs-coop domain kararı (§5) + karar
   doğrultusunda üyelik-yönetimi yüzeyi.
3. B1 kararı: creation-path definer'larına taşıma (önerilen) veya
   yazılı kabul — ikinci coop'ta insider-risk yüzeyi büyür.
4. HTTP üyelik-yönetimi endpoint'i eklenirse
   `cooperative_memberships` audit trigger'ı.
5. B4 kapsam boşluğu: en az bir domain akışının app-pool'lu HTTP
   smoke'u CI'a eklensin.

**NO-GO koşulu yok** — bulunan rezidüeler yetkili-içeriden + tam
audit'li sınıf; hiçbiri cross-tenant veya atıf-sahtesi değil.

---

*Audit yalnız okuma + disposable DB sondaları ile yürütüldü;
production kodu, migration veya checksum değiştirilmedi; probe
satırları temizlendi (users FK'si audit-kaynaklı bir satırı korudu —
disposable DB'de zararsız).*
