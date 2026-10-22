# M1-P4 — BAĞIMSIZ TENANT GÜVENLİK KAPANIŞ AUDIT'İ

Tarih: S5 sonrası. Kapsam: `kooperatif_app` runtime principal'ının
gerçek DB yetki yüzeyi, RLS politikaları, SECURITY DEFINER sınırı,
finansal invariantlar ve test kalitesi. Yöntem: salt-okunur inceleme —
raporlar kullanılmadı; tüm kanıtlar `kooperatif_test` (v51) canlı
katalog sorguları ve kaynak koddan bağımsız doğrulandı.

Önemli düzeltme: audit sırasında ilk bakışta "tek-kolon FK"
sanılan ilişkilerin aslında `*_coop_fk` kompozit filosuyla
`(cooperative_id, X_id) → parent(cooperative_id, id)` biçiminde
korunduğu doğrulandı — business-grafting yüzeyi neredeyse kapalı.

## 1. Doğrulanan mimari (bağımsız yeniden-kurulum)

- **Kimlik/oturum:** `user_sessions` tamamen kapalı (sıfır grant);
  tüm oturum işlemleri credential-bound definer'lar. `users` sadece
  kolon-kısıtlı SELECT + `UPDATE(status,updated_at)`.
- **Mühür zinciri:** `app_open_request_context` → user_seal;
  `app_stamp_cooperative` → üyelik doğrulaması + ctx_seal;
  `app_ctx_ok()`/`app_user_ctx_ok()` HMAC doğrular (owner-secret).
  `app_seal_of`, `app_session_owner`, `app_verify_membership`
  EXECUTE'a kapalı — doğru.
- **RLS filosu:** 42 tablo FORCE-RLS / 82 politika;
  `cooperative_id` taşıyan her tablo kapsamda (sıfır istisna).
- **Kompozit FK filosu:** `*_coop_fk` — 60+ kısıt; kooperatif-sahip
  tablolara referansların tamamı (istisna: `role_permissions.role_id`
  ve diskriminatör `entry_kind` kolonları).
- **Definer yüzeyi:** 26 `app_*` fonksiyonu EXECUTE-verilmiş;
  gövde incelemesi: hepsi ya token-hash-bound ya seal-doğrulamalı;
  allocator/reserve scope-eşitliği zorlar; `app_admin_revoke_user_sessions`
  `users.manage` + member_visibility yeniden-doğrular.
- **Pool hijyeni:** `before_acquire` RESET (GUC + ROLE);
  `kooperatif_app` rol üyeliği yok, BYPASSRLS/superuser değil.
- **Audit:** `security_events` tamamen kapalı; tek yazım yolu
  definer'lar (`app_ctx_audit` mühürlü, `app_session_audit_*`
  oturum-bağlı whitelist, `app_audit_login_failed` sabit).

## 2. Bulgular — invariant bazında

### F1 — CRITICAL: `role_permissions` DB sınırı yok

- Kanıt: `role_table_grants` → `DELETE,INSERT,SELECT`;
  `pg_class` → `relrowsecurity=f`; FK yalnız düz `role_id → roles(id)`;
  tabloda `cooperative_id` kolonu yok; trigger yok.
- **Oturumsuz exploit:** `kooperatif_app` bağlantısıyla, hiç mühür/
  oturum olmadan:
  - `SELECT role_id, permission_id FROM role_permissions` → tüm
    kooperatiflerin rol↔izin haritası (platform dahil) — global
    enumeration.
  - `DELETE FROM role_permissions` → tüm rollerin tüm izinleri
    silinir → **global yetkilendirme kesintisi** (herkes 403).
  - `DELETE ... WHERE role_id=<X>` → hedef rolün izinleri → hedef
    kooperatif kilitlenir.
  - `INSERT (role_id=<kendi_rolüm>, permission_id=<roles.manage>)` →
    **self-escalation** (izin id'leri `permissions` SELECT ile
    enumerable; kendi rol id'si `app_session_role_summaries_scoped`
    ile edinilir).
  - `INSERT (role_id=<NULL platform rolü>, …)` → platform
    yetkisine grafting.
- Ön-koşul: yalnız `kooperatif_app` DB credential'ı — oturum bile
  gerekmiyor. Etki: yetki sistemi bütünlüğü + platform geneli DoS.
- **R1 remediation (migration 0052):** INSERT/DELETE grant'leri
  kaldırıldı (yalnız SELECT kaldı — mühürlü join okumaları için);
  tüm yazımlar `app_role_permissions_set` definer'ına taşındı —
  sealed ctx + `roles.manage` + parent-rol kapsamı + lockout
  yeniden-doğrulaması DB içinde. Kanıt:
  `r1_role_permissions_direct_write_denied`,
  `r1_sealed_member_has_no_rbac_mutation_path`,
  `r1_cross_cooperative_rbac_mutation_denied` (gerçek app bağlantısı).

### F2 — CRITICAL: `ura`/`roles` doğrudan yazım izin modelini atlar

- Kanıt: `user_role_assignments` INSERT/DELETE + `roles`
  INSERT/UPDATE grant'leri; app politikası yalnız coop-eşitliği +
  mühür denetler — **izin düzeyi denetimi RLS'de mümkün değil**,
  uygulama katmanına güveniyor.
- **Exploit (aktif üye + sealed tx + raw SQL):**
  `INSERT INTO user_role_assignments (cooperative_id, user_id, role_id)
   VALUES (<own_coop>, <me>, <admin_role_id>)` — kompozit FK'ler
  `(coop,user)→memberships`, `(coop,role)→roles` kendi verileriyle
  sağlanır; WITH CHECK geçer → **herhangi bir üye kendi kooperatifinin
  admin rolünü kendine atar; `roles.manage` kontrolü baypas.**
- `DELETE FROM user_role_assignments WHERE cooperative_id=<own>
  AND user_id=<other_admin>` → kendi-coop kilitlenmesi (DoS).
- `INSERT INTO roles (cooperative_id=<own>, …)` → sınırsız rol
  üretimi (F1 ile zincirlenince tam yetki).
- Not: bu, S5 testinin meşru saydığı davranışın ta kendisi
  (`s5_sealed_scope_isolates` in-scope insert'i assert ediyor) —
  test, implementasyon varsayımını tekrarlıyor; güvenlik
  invariant'ı "üye kendine admin atayamaz" DB katmanında SAĞLANMIYOR.
- Ön-koşul: geçerli oturum (kendi coop'u) + DB erişimi veya SQLi.
- **R1 remediation (migration 0052):** `roles`/`ura` yazım grant'leri
  kaldırıldı (salt-SELECT); yazımlar `app_role_create/update/
  set_status` ve `app_user_roles_set` definer'larında — her biri
  mühür + `roles.manage`/`users.manage` + satır kapsamı + son-admin
  korumasını DB içinde yeniden doğrular. `app_rbac_verified_scope`
  sealed ctx'ten scope'u türetir (caller-supplied coop asla
  kabul edilmez). Kanıt: `r1_sealed_member_has_no_rbac_mutation_path`,
  `r1_lockout_protection_inside_boundary`,
  `r1_forged_guc_cannot_invoke_mutators`.

### F3 — HIGH: `users` UPDATE(status)/SELECT kapsamsız

- Kanıt: `column_privileges` → SELECT(7 kolon), UPDATE(status,
  updated_at); `users` RLS'siz (cooperative_id'siz global kayıt).
- **Exploit:** `UPDATE users SET status='disabled'` → **global
  kullanıcı kilitlemesi** (tüm kooperatifler, oturumsuz);
  `status='active'` → pasif hesap reaktivasyonu. `SELECT
  username,display_name,last_login_at` → **global kullanıcı
  enumeration** (cross-tenant üye listesi + aktivite).
- Meşru kullanım `set_user_status`'te `member_visibility` WHERE'i
  var — ama raw SQL o filtreyi atmaz.
- **R1/R2 reproduce (kooperatif_test, gerçek `kooperatif_app`
  bağlantısı):** cross-coop `UPDATE users SET status='disabled'` →
  `UPDATE 1` başardı; `SELECT count(*) FROM users` → 1102 satır
  enumerate edildi; `SELECT password_hash` → zaten 42501.
- **REMEDIATED (migration 0053):** `users` ENABLE+FORCE RLS; app
  politikası FOR SELECT — mühürlü kapsam yalnız kapsam-üyesi satırları
  görür (üyelik her statüde sayılır = member_visibility eşdeğeri),
  user-seal NULL kolu yalnız sıfır-kooperatif pre-tenancy'de açılır.
  `UPDATE(status,updated_at)` revoke; SELECT 4 kolona daraldı
  (id, username, display_name, status). Tek mutasyon kulvarı
  `app_user_set_status`: `app_rbac_verified_scope('users.manage')` +
  üyelik görünürlüğü + `app_admin_path_count` son-admin koruması,
  tek statement'ta atomik. Post-fix aynı bağlantı: UPDATE → 42501,
  count(*) → 0, definer → 42501. Kanıt: `r2_users_direct_write_denied`,
  `r2_users_enumeration_is_scope_bound`, `r2_user_set_status_definer_paths`,
  `r2_user_disable_lockout_inside_boundary`,
  `r2_forged_guc_cannot_set_user_status`,
  `r2_never_bootstrapped_lane_opens_then_closes`,
  `r2_shared_user_status_is_global_by_membership`.
- **Kalan sınır (belgelendi):** `users.status` global kimlik
  özniteliği — birden çok kooperatife üye kullanıcı, üyesi olduğu
  HERHANGİ bir kooperatifin admin'i tarafından kapatılabilir (etki
  globaldir). `member_visibility` semantiği korundu; kooperatif-bazlı
  status ayrımı domain-spec değişikliği gerektirir — R2 kapsamı
  dışında.

### F4 — HIGH: Finansal tarihte kullanılmayan DELETE/UPDATE yüzeyi

- Kanıt: uygulama kodunda `DELETE FROM` yalnız 4 tabloda
  (`role_permissions`, `ura`, `assessment_rules`, `periods` — son
  ikisi draft temizliği); buna karşılık app **~37 tabloda DELETE**
  ve tümünde UPDATE tutuyor.
- **Exploit (sealed tx):** `DELETE FROM payments WHERE
  cooperative_id=<own>` → post'lanmış finansal kayıtlar silinir;
  `UPDATE account_movements SET status='reversed'` → türetilmiş
  bakiyeler manipüle edilir. "Posted finansal işlem asla
  yıkıcı silinmez" invariant'ı DB katmanında karşılanmıyor —
  yalnız uygulama disiplinine bağlı.
- Etki kendi-coop ile sınırlı (RLS) ama tarih-bütünlüğü kaybı
  finansal sistem için kritik kabul edilmelidir.
- **REMEDIATED (R3, migration 0054):**
  `0054_r3_financial_grant_boundary.sql` ile yüzey üç katmanda
  daraltıldı —
  (a) DELETE yalnız `periods` + `assessment_rules`'ta kaldı ve orada
  bile RESTRICTIVE politikalar silmeyi **draft dönemlere** (ve draft-
  ebeveynli kurallara) kısıtlıyor;
  (b) UPDATE hiç kullanılmayan 7 tabloda (assessments,
  assessment_share_sources, investment_disposals,
  investment_disposal_proceeds, share_events, families,
  governance_votes) tamamen kaldırıldı — runtime için append-only;
  (c) kalan 30 tabloda UPDATE, repository'lerin yazdığı kolon
  listesine daraltıldı (amount/payer/account/cooperative_id/
  created_by gibi tarihsel alanlar tamamen yazılamaz).
  Terminal-yaşam-döngüsü tablolarında (payments, account_movements,
  account_transfers, income/expense_entries, payment_allocations,
  credit_applications, shareholder_credits, investment_fundings/
  incomes/valuations, investments, share_return_settlements/returns,
  social_aid_donations/disbursements/funds, governance_bodies)
  RESTRICTIVE FOR UPDATE guard'ları tek-ileri geçişi zorluyor
  (active/posted/recorded/pending → reversed/cancelled/closed/
  disposed/finalized); terminal satır frozen, aynı tx'te bile geri
  alınamaz. RESTRICTIVE FOR INSERT guard'ları terminal-status'lü satır
  basmayı engelliyor.
  Kanıt (m1p4_rls_pilot.rs, gerçek `kooperatif_app`): pre-fix repro
  `UPDATE payments SET amount=5` → UPDATE 1, `DELETE FROM payments` →
  DELETE 1; post-fix hepsi `42501` / 0 satır.
  `r3_posted_financial_history_is_immutable`,
  `r3_terminal_transitions_are_forward_only`,
  `r3_terminal_rows_cannot_be_inserted`,
  `r3_deletion_is_draft_confined`,
  `r3_operational_edits_still_work_attribution_frozen`,
  `r3_cross_cooperative_financial_mutation_denied`.
- **Kalan sınır — REMEDIATED (R3B, migration 0055):**
  `0055_r3b_lifecycle_definers.sql` ile tüm lifecycle/terminal
  geçişleri 39 dar, izin-doğrulamalı SECURITY DEFINER'a taşındı;
  karşılık gelen kolon UPDATE grant'leri tamamen revoke edildi.
  Her definer `app_rbac_verified_scope('<module>.manage')` ile mühürlü
  bağlam + aktif üyelik + rol iznini DB içinde yeniden doğrular ve
  scope'u mühürden türetir (çağrıcı `cooperative_id` güvenilmez).
  `app_movements_reverse_of_source` izni `source_type`'tan türetir;
  helper'lar EXECUTE'suz. Mühürsüz owner kulvarı (migrasyon/CLI/
  fixture) kısıtsız; app-rolü için bypass yok.
- **Eksik-paket sınırı — REMEDIATED (R3C, migration 0056):**
  `0056_r3c_atomic_reversal_packages.sql` ile izinli bir aktörün tek
  bileşen definer çağırıp telafi muhasebe etkisini atlatabilmesi
  (payment `reversed` + hareket `active` + bakiye değişmeden)
  kapatıldı: reversal/finalize/disposal/close paketleri 24 dar
  `app_*_tx` definer'da atomikleştirildi; bileşen definers
  PUBLIC + `kooperatif_app` EXECUTE'undan düşürüldü. Sealed kulvarda
  `p_by` mühürden türetilir (sahte atıf yok); governance sayım
  snapshot'ı definer içinde hesaplanıyor.
  Kanıt (v55 residual, gerçek `kooperatif_app`): `payments.manage`
  izinli üye mühürlü tx'te `app_payment_reverse`'i tek başına çağırdı
  → payment `reversed` ama hareket `active`, bakiye 100 — ihlal
  edilen invariant: reversal, kaynak satır + telafi hareketi etkisini
  atomik yapmalı.
  Kanıt (v56 post-fix): `app_payment_reverse` → **42501**
  (bileşen EXECUTE'suz); `app_payment_reverse_tx` tek çağrıda
  payment+allocation+movement+credit etkisini atomik uygular; sealed
  scope olmadan ret; izinsiz mühürlü üyede 42501; replay idempotent
  (`replayed=true`, ikinci etki yok); önkoşul hatasında tüm etkiler
  rollback; eşzamanlı iki reversal → biri uygular, diğeri replay;
  cross-coop satır 0 etki; sahte `p_by` mühürlü aktörle
  üzerine yazılır.
  Testler: `r3c_incomplete_reversal_package_is_impossible`
  + `m1p4_tenant_context` (component EXECUTE denied, package EXECUTE
  granted) + paket-sınırına uyarlanmış R3/R3B suiti — `m1p4_rls_pilot`
  107/107.
- **Bilinen sınır (belgelendi):** `account_repo::reverse_movements_of_source`
  Rust yardımcısı üretim kodunda kullanılmıyor; yalnız owner-kulvarı
  test fixture'ı çağırıyor ve `kooperatif_app` için EXECUTE kapalı
  (runtime erişimi yok). Zorunlu uygulama-audit'i yine route-katmanı
  sorumluluğunda (F5 kapsamında belgelenen mimari sınır); paketler
  finansal tarihi DB'de garantili kılar, business-audit satırı
  HTTP akışının sorumluluğunda kalır.

### F5 — MEDIUM: Audit delikleri

- `app_ctx_audit(event_type, metadata)` — attribution mühürlü ama
  **event_type serbest metin**: sealed ctx'te sahte
  `user_roles_change`/`lockout_prevented` satırları basılabilir
  (kendi atfıyla, kendi coop'unda — audit manipülasyonu).
- Zorunlu audit yalnız uygulama katmanında atomik: raw SQL ile
  sealed-scope'ta yapılan herhangi bir INSERT/UPDATE/DELETE
  security_events'siz kalır → **audit-bütünlüğü DB'de garantili
  değil** (bilinen mimari sınır; belgelenmeli).

**R4 kanıtı — CONFIRMED:** `kooperatif_app` sealed tx'te
`SELECT app_ctx_audit('payment_reversed','{}')` başarıyla sahte satır
basabiliyordu (v56). **REMEDIATED (0057):**
`app_ctx_audit` dar whitelist'e bağlandı (yalnız context/security
telemetry tipleri); 40+ business event trigger-katmanında
`app_audit_transition` ile mint ediliyor — aynı tx, aktör/coop mühürden,
rollback'te satır da kaybolur; `app_write_guard` (BEFORE INSERT)
izinsiz doğrudan business yazımını 42501 ile keser; ~90 route-katmanı
`record_tx` çağrısı trigger'a devredilerek çift-audit kapatıldı.
Best-effort telemetry (`record_scoped`/`record_best_effort`) ve
bootstrap/auth akışları değişmedi.

### F6 — LOW: Enumeration yüzeyi

- `cooperatives` SELECT (isim, legal_name, status — tüm tenant'lar),
  `installation_state` SELECT, `app_resolve_bootstrap()`
  (bootstrap coop id+adı), `role_permissions` SELECT.
- Tek başına bilgi sızıntısı; hedefli saldırıyı kolaylaştırır
  (grafting için gerekli uuid'lerin bir kısmı buradan).

**R4 kanıtı — CONFIRMED:** v56'da `kooperatif_app`'ten
`SELECT count(*) FROM cooperatives→5, installation_state→1,
permissions→31, role_permissions→34` dönüyordu.
**REMEDIATED (0057):** dört tablonun SELECT grant'i revoke;
uygulama okumaları `app_session_permissions_scoped`,
`app_permission_catalog`, `app_role_permission_keys`,
`app_resolve_bootstrap`, `app_bootstrap_completed`,
`app_cooperatives_exist`, `app_is_bootstrap_scope`,
`app_session_permissions_bootstrap`,
`app_session_role_summaries_bootstrap`, `app_admin_path_count`
definer'larına taşındı (Rust: `authz.rs`/`rbac.rs`/`tenant/repo.rs`
rewired). `roles`/`user_role_assignments`/`users` SELECT politikaları
revokeli tablolara bağımlıydı → definer-helper'larla yeniden yazıldı.
Canlı doğrulama: dört tabloda `permission denied`; definer'lar EXECUTE
ile dar sonuç döndürüyor (authz_cooperative 11/11).

### F7 — LOW: Polimorfik referanslar

- `account_movements.source_type/source_id` FK'siz — kendi-scope'ta
  uydurma atıf mümkün; cross-coop etki yok (referans satırı zaten
  görünmez). Mimari sınır.

**R4 kanıtı — CONFIRMED (kendi-coop uydurma atıf):** v56'da sealed
tx'te `account_movements` INSERT ile var olmayan/cross-coop/yanlış
domain source atfı yapılabiliyordu. **REMEDIATED (0057):**
`app_movement_source_pre` (BEFORE: `source_type`/hesap/tutar/yön
doğrulaması — kooperatif-kaynaklı alanlar sealed tx'te ezip
kullanıcı-girdi atıfını reddeder) + `app_movement_source_check`
(AFTER, INITIALLY DEFERRED CONSTRAINT TRIGGER: kaynak satırı aynı
coop'ta, doğru domain, `account_movement_id` geri-bağlantısı,
yön+tutar+hesap eşleşmesi; `created_by` yalnız mühürlü aktörün
kendi tx'inde bastığı satırı kabul eder — sahte `created_by` atfı
trigger'da reddedilir). Cross-coop/invalid-source ret kanıtı
`r4_audit_integrity_and_residual_closure` testinde. Tarihsel
fixture/owner-kulvarı kayıtları korundu.

## 3. PASS doğrulanan invariantlar

| Invariant | Kanıt |
|---|---|
| Cross-coop business okuma/yazma | 42 FORCE-RLS tablo; sealed-scope politikalar; S-testleri (arbitrary SQL, gerçek app conn) |
| Cross-coop FK grafting (business) | `*_coop_fk` kompozit filosu — 60+ kısıt, sistematik sorgu: kooperatif-sahip tabloya giden tek kopmamış kolon `role_permissions.role_id` (+zararsız `entry_kind`) |
| Oturum/aktör sahteciliği | HMAC seal (user\|session\|coop\|txid), owner-only secret, tx-freshness; sahte GUC testleri |
| Definer parametre manipülasyonu | Gövde incelemesi: hash-bound veya seal-doğrulamalı; allocator/reserve scope-eşitliği; admin_revoke `users.manage`+member_visibility yeniden-doğrular |
| Bağlantı-yeniden-kullanım sızıntısı | `before_acquire` RESET GUC+ROLE; testler |
| Bootstrap/NULL kulvarı | NULL arm: marker+flag yeniden-doğrulama VEYA kanıtlanmış pre-tenancy; post-bootstrap NULL erişilemez (s5 testi) |
| Üyelik tablosu | `cooperative_memberships` SELECT-only app + sealed-scope politika; NULL üyelik app-kulvarında okunamaz |
| Belge numaralandırma | Counters owner-only; allocator/reserve `app_ctx_ok()`+coop-eşitliği |
| Session credential izolasyonu | `user_sessions`/`app_secrets`/`_sqlx_migrations` sıfır grant |
| Son-admin koruması (uygulama katmanı) | `administration_path_count` + advisory lock + scoped testler — DB katmanında raw DELETE ile aşılabilir (F2/F3'e bağlı) |
| Realtime içerik sızıntısı | `kooperatif_notify_domain` payload = domain tag; revalidation definer-bound |
| Finansal atomiklik | Komutlar `ctx.open_tx` tek-tx; derived balance/settled; composite FK'ler cross-coop ledger karışmasını engeller |

## 4. Test kalitesi audit'i

- S-testleri gerçek `kooperatif_app` bağlantısı + arbitrary SQL —
  doğru model ve iyi derinlik (mühür sahteciliği, geçici grant
  altında RLS kanıtı, NULL kulvarları, abort-izolasyonu).
- Eksik negatif kapsam:
  1. `role_permissions` graft/strip — test yok.
  2. `users` kapsamsız UPDATE/SELECT — test yok.
  3. `ura` self-assignment (üye → kendi admin rolü) — test yok;
     üstelik mevcut test bu vektörü meşru davranış olarak assert ediyor.
  4. Finansal tablolarda raw DELETE — test yok.
  5. `app_ctx_audit` serbest event_type — test yok.
  6. Bootstrap-scope NULL `ura` insert'inin `user_id`'si FK
     dışında üyelik-denetimsiz — bilinçli tasarım ama test
     senaryosu olarak belgelenmeli.
- Kapsam değerlendirmesi: satır-kapsamı invariant'ları iyi;
  **izin-düzeyi invariant'ları DB katmanında hiç test edilmemiş**
  (çünkü orada uygulanmıyorlar — bulgularla tutarlı).

## 5. Mimari uygunluk

- Katman ayrımı temiz: extractor → sealed tx → repo; trust-boundary
  definer'larda.
- `scoped_visibility` semantiği hem rbac.rs SQL fragment'ında hem
  0051 politikalarında var — **çift kaynak**: semantik drift riski
  (LOW — politikalar DB'de authoritative ama Rust tarafı kopyası
  sorgu-filtresi olarak kalıyor).
- `authz.rs`'deki pool-bazlı ölü yardımcılar (`effective_permissions*`,
  `require`, `active_role_summaries*`) hâlâ derleniyor — gelecekte
  yanlışlıkla canlı yola bağlanma riski (LOW — kaldırma veya
  işaretleme önerilir).
- **AÇIK MİMARİ KARAR (closure item, R2/R3'te değiştirilmedi):**
  `users.status` **global** kimlik özniteliği olarak kaldı — bir
  kullanıcının birden çok kooperatifte üyeliği varsa, herhangi birinin
  admin'i kullanıcıyı global olarak askıya alabilir/aktifleştirebilir.
  Alternatif — kooperatif-bazlı üyelik süspansiyonu
  (`cooperative_memberships.status` üzerinden) — domain-spec
  değişikliği gerektirir: hangi modelin doğru olduğu (global kimlik
  kapatma mı, coop-scoped üyelik askısı mı) product/domain kararıdır
  ve M1-P4 closure öncesi ayrıca karara bağlanmalıdır. Semantik
  bilinçli olarak değiştirilmedi (docs/16, 18'e göre mevcut davranış
  korundu; bkz. F3 remediation notu).

## 6. Remediasyon önerisi (bağımlılık sırasıyla)

*(R1+R2 sonrası durum: maddeler 1–3 migration 0052 ile kapatıldı —
`role_permissions` salt-SELECT + `app_role_permissions_set`;
`ura`/`roles` yazımları definer sınırında. Madde 2 (`users` kapsamı)
migration 0053 ile kapatıldı — `app_user_set_status` + FORCE RLS +
SELECT 4 kolona daraltıldı. Madde 4–6 açık.)*

1. **`role_permissions` sınırı:** ya `roles` join'li RLS
   (EXISTS subquery ile aynı scope ifadesi), ya — tercihen —
   doğrudan grant'lerin kaldırılıp `app_role_set_permissions`
   benzeri mühür+izin-doğrulamalı definer'a taşınması.
2. **`users` kapsamı:** `UPDATE(status,updated_at)` grant'ini
   kaldırıp `app_admin_set_user_status` definer'ına taşı
   (users.manage + member_visibility + lockout yeniden-doğrulama —
   `app_admin_revoke_user_sessions` deseni hazır). SELECT için
   üyelik-türevli politika değerlendir.
3. **`ura`/`roles` yazımları:** doğrudan grant'leri kaldırıp
   yazımları sealed-ctx `roles.manage` doğrulamalı definer'lara
   taşı — VEYA politika WITH CHECK'e `app_has_admin()` benzeri
   definer predicate ekle (definer owner olarak ura/roles okur →
   recursion yok; S4'teki app_ctx_ok deseniyle uyumlu).
4. **Grant hijyeni:** uygulamanın DELETE kullanmadığı ~33 tabloda
   DELETE revoke; UPDATE'ı kullanılan kolonlarla sınırla
   (mümkünse kolon-grant). **DONE (0054)** — DELETE yalnız
   periods/assessment_rules'ta (draft-confined), UPDATE 30 tabloda
   kolon-bazlı, 7 tabloda tamamen kaldırıldı, terminal tablolara
   geçiş-guard + insert-live politikaları eklendi.
5. `app_ctx_audit` event_type whitelist veya kabul+belgeleme.
   **DONE (0057)** — dar whitelist; business tipleri trigger'a
   devredildi.
6. Enumeration azaltımı: `cooperatives`/`installation_state`
   SELECT'ini definer arkasına almayı değerlendir.
   **DONE (0057)** — `cooperatives`, `installation_state`,
   `permissions`, `role_permissions` SELECT revoke + definer
   reader'lar; bağımlı SELECT politikaları definer-helper'lı.
7. `account_movements` polimorfik kaynak bütünlüğü.
   **DONE (0057)** — BEFORE pre-check + deferred source-check
   constraint trigger'ı.

## 7. Karar

- **M1-P4 closure: NO-GO.** Satır-kapsamlı tenant izolasyonu sağlam
  ve kanıtlı. İki CRITICAL bulgu (F1, F2) migration 0052 ile, HIGH
  F3 migration 0053 ile, HIGH F4 migration 0054 ile kapatıldı
  (finansal tarih DB katmanında artık silinemez / tarihsel alanları
  yazılamaz / terminal geçişler tek-yönlü); F5/F6/F7 migration 0057
  ile kapatıldı (transition-audit trigger katmanı + whitelist'li
  `app_ctx_audit` + yazım-guard'ları; enumeration tabloları revoke +
  definer reader'lar; deferred polimorfik-kaynak bütünlüğü). R3C
  24 paketinin tamamı audit-atomikliği trigger katmanından sağlanır
  (satır-işlem aynı tx; audit başarısızlığı işlemi rollback'ler).
  Açık kalan: `users.status` global-vs-coop domain kararı (bölüm 5).
  Nihai closure değerlendirmesi — backup-readiness kapısı dahil —
  ayrı onay bekliyor.
- **İkinci kooperatif aktivasyonu: NO-GO.** Doğrudan cross-tenant
  etki üreten F1/F3 kapatıldı; F4'ün kalan lifecycle-write sınırı
  ve F5/F6/F7 kapatıldı — ancak nihai karar backup-readiness
  gate'ine ve açık domain kararına bağlı.
- Not: tüm exploit'ler `kooperatif_app` DB credential'ına veya
  app-bağlantısında SQLi'ye dayanıyor — HTTP katmanındaki davranış
  temiz; bulgular defense-in-depth katmanındaki boşluklardır,
  ancak bu katman M1-P4'ün tam konusudur.
