# M1-P6 — STEP 1: KOOPERATİF YÖNETİMİ KEŞİF VE GAP DENETİMİ

Tarih: 2026-10-18 · Taban: migration 0066 · Tip: SALT-OKUNUR denetim (kod/migration/DB değişikliği yok)

## 1. Yetenek envanteri

| Yetenek | Durum | Kanıt |
|---|---|---|
| Kooperatif oluşturma | **KISMEN** | Yalnız CLI `create-cooperative` → `repo::create_cooperative` her zaman `status='provisioning'` + `business_enabled=false` üretir (`main.rs:593`, `tenant/repo.rs:57`). HTTP route yok. |
| Kooperatif aktivasyonu (provisioning→active, business_enabled) | **EKSİK** | Hiçbir migration/src'de `UPDATE cooperatives` yok — aktivasyon yolu sıfır (definer, CLI, HTTP hiçbiri). B5 testi aktivasyonu ancak doğrudan SQL ile yapabildi. `0018` satır 27-29'daki tasarım notu: "created false and stay unreachable" — bilinçli gate ama açacak mekanizma yazılmadı. |
| Kooperatif suspend/archive/reaktivasyon | **EKSİK** | CHECK: `('provisioning','active','suspended','archived')` — durum makinesi şemada var, yazan kod yok. |
| Kooperatif profil/ayar güncelleme | **EKSİK** | Kolonlar: `name, legal_name, status, is_bootstrap, business_enabled` — profil alanı ve API yok. |
| Kooperatif listeleme (kullanıcı) | **TAMAM+TESTLİ** | `GET /api/cooperatives` → `list_user_cooperatives` (S2A definer), `me` de `cooperatives[]` döner. `tenant_kernel` testleri. |
| Kooperatif listeleme (platform admin) | **EKSİK** | Tüm kooperatifleri listeleyen platform endpoint'i yok. `kooperatif_app`'in `cooperatives` tablosunda grant'i yok (doğru) — definer gerekir. |
| Kooperatif context çözümü | **TAMAM+TESTLİ** | `GET /api/auth/cooperative-context` (TenantCtx kanıt endpoint'i). |
| Kooperatif değiştirme (switch) | **TAMAM+TESTLİ** | `POST /api/auth/cooperative` → doğrulanmış üyelik + `app_session_set_cooperative` definer'ı + `cooperative_context_switched` audit + 0066 authz invalidation + stale-WS kapanışı. |
| Üyelik oluşturma (üye ekleme) | **KISMEN** | Yalnız CLI `add-member` → `repo::add_member` plain INSERT (owner-lane). `kooperatif_app`'in memberships INSERT grant'i YOK (FORCE RLS + SELECT-only) → HTTP üye ekleme için sealed definer gerekiyor. |
| Üyelik yaşam döngüsü (suspend/reactivate/end) | **TAMAM+TESTLİ** | `POST /api/users/{id}/membership-{suspend,reactivate,end}` → `app_membership_set_status` (0060) — sealed-scope (`app_rbac_verified_scope`), per-coop admin-path koruması, `ended_at` korur, tetik audit `cooperative_membership_{suspended,ended,reactivated}` basar. |
| Üye/kullanıcı listeleme (scoped) | **TAMAM+TESTLİ** | `GET /api/users` — scoped görünürlük predikatları, membership durumunu döner. |
| Kullanıcı (kimlik) oluşturma | **KISMEN** | CLI `create-user` only — HTTP `/api/users` POST yok. |
| Kullanıcı daveti | **EKSİK** | Davet/invitation kavramı hiç yok (parola CLI'de veriliyor). |
| Varsayılan/aktif kooperatif | **TAMAM** | `user_sessions.active_cooperative_id` (UX default — M1-K1), switch endpoint'i yazar. |
| İlk admin atama | **KISMEN** | CLI `add-member` + `grant-role` — ama `grant-role`'da `--cooperative` yok; NULL-scope rol adıyla çözümlenir, coop-scope rol ataması HTTP `PUT /api/users/{id}/roles` ile mümkün (scoped). Atomik "kooperatifi aktive et + ilk admini ata" akışı yok. |
| Son etkin admin koruması | **TAMAM+TESTLİ** | `app_admin_path_count(scope)` (coop) + `app_platform_admin_path_count()` (NULL-scope `roles.manage`) — `lockout_prevented` check_violation. |
| Platform yönetimi | **KISMEN** | `/api/platform/users/{id}/disable|enable` + `/api/platform/audit/events` var; platform tarafında kooperatif yönetim yüzeyi yok. Platform admin = aktif NULL-scope `users.manage`. |
| Coop-spesifik izinler | **EKSİK** | Katalogda `cooperatives.*` / `memberships.*` yok; roadmap P6 `coop_members.manage` öneriyordu. Üyelik yazma bugün `users.manage` kapsamında. |
| İş uçları çoklu-koop | **LEGACY → REFACTOR** | `BusinessAuth` bootstrap-pinli: 191 kullanım, ~14 router + WS + audit-scoped. `x-coop` non-bootstrap→403. `TenantCtx` genel doğrulanmış-scope extractor'ı mevcut ama tek tüketicisi `cooperative_context` probe'u. |

## 2. Yaşam döngüsü gerçekleri

- **Oluşturan:** yalnız owner-lane CLI (ops). HTTP yok.
- **Aktive eden:** **kimse** — mekanizma mevcut değil.
- **Suspend/deactivate:** yok; şema `suspended`/`archived`'ı kabul eder.
- **İlk admin:** CLI kombinasyonu (add-member + grant-role); atomik değil.
- **Son admin:** `lockout_prevented` DO-assert'leri membership + rol mutasyonlarında.
- **Suspend'de session/WS:** suspend → `verify_business_membership` `cooperative_status='active'` şartı → iş uçları 403. WS: `cooperatives` tablosunda rt_authz trigger'ı YOK → proaktif kapanma olmaz; ilk sinyalde revalidate kapatır (fail-closed ama tembel). **P6'ya taşınacak gereksinim.**
- **Reaktivasyon + tarihçe:** status toggle'ları satırı silmez; audit atıfı korunur.

## 3. Kimlik/üyelik ayrımı — doğrulandı (0060 + B5 kanıtları)

- Tek kimlik çok kooperatif: `cooperative_memberships (coop,user)` PK + `list_user_cooperatives` — EVET.
- Üyelik durumu coop başına bağımsız — EVET.
- Rol/izin coop-scope: `roles.cooperative_id` + kompozit FK + `app_rbac_in_scope` (NULL rol yalnız bootstrap'te geçerli) — EVET.
- Coop admini başka coop'u yönetemez — `app_rbac_verified_scope` sealed-GUC türetimi, parametre kabul etmez — EVET.
- Global disable yalnız platform — `app_platform_user_set_status` + `app_is_platform_admin` — EVET.
- Üyelik sonu tarihçeyi silmez — `ended_at` işaretli, satır kalır — EVET.
- Switch stale-yetki tutmaz — 0066 + revalidate scope karşılaştırması — EVET.

## 4. WS scope sınırlaması — kök neden ve öneri

**Kök neden:** WS upgrade `BusinessAuth` kullanır; `BusinessAuth` tasarım gereği bootstrap-pinli (TÜM business yüzeyi gibi). `x-cooperative-id` non-bootstrap→403; headerless→bootstrap. Sorun WS'e özgü değil: tüm iş yüzeyi bootstrap'e kilitli (191 referans).

**Öneri (P6-B4):** WS scope'u TenantCtx-semantiğiyle genelleştirilir:

1. Handshake'te `scope := session.active_cooperative_id` (kullanıcı talimatı: session'dan türet).
2. `x-cooperative-id` header'ı varsa: **session scope'undan farklıysa → 403** (sessiz override yok); eşitse devam — header ancak onaylama rolü oynar, asla yetki kaynağı olmaz.
3. Üyelik + `business_enabled` + coop `active` doğrulaması `verify_business_membership` ile (mevcut).
4. Revalidate (B5/0066): switch → `active_cooperative_id` değişimi → authz → bound≠active → kapanış. Zaten inşa edildi.
5. NOTIFY/HMAC/replay/multi-instance mimarisi değişmez — yalnız `scope` artık bootstrap dışı bir coop olabilir; listener'daki `Some(coop)` filtreleme ve per-listener dedup coop-agnostiktir.

**Sonuç uyarısı (K1'e bağlı):** session-türetimli scope, iki sekmenin AYRI kooperatifte WS dinlemesini engeller (session paylaşımlı). HTTP tarafı per-tab başlığı destekler (M1-K1). Gerçek per-tab çok-coop realtime istenirse per-tab session ayrımı gerekir — daha büyük karar, K1'de sunuldu.

## 5. Güvenlik sınırları — P6 API'leri için bulgular

- `cooperatives` tablosu: **grant'siz** (RLS'siz ama app'a hiç grant yok → 42501) — tüm erişim definer yoluyla, doğru mimari. P6 okuma/yazma definer'ları aynı kalıbı izlemeli.
- `cooperative_memberships`: FORCE RLS + `app_scope` (GUC=eşitlik), app'a yalnız SELECT. **INSERT grant'i yok** → üye ekleme definer'ı şart (`app_membership_create`, scope'u `app_rbac_verified_scope`'tan türeten — asla parametre olarak coop almayan).
- **Eskalasyon vektörleri (öncelikli):**
  1. Üye-ekleme definer'ı coop'u parametre alırsa = cross-coop üye ekleme açığı → `app_membership_set_status` kalıbı (sealed scope) zorunlu.
  2. Platform coop-listesi coop-scope izinle açılırsa = isim/id enumeration sızıntısı → NULL-scope `cooperatives.manage` zorunlu.
  3. Aktivasyon + ilk-admin aynı tx'te değilse yetkisiz-provisioning-coop kalır → atomik paket.
  4. Coop suspend'de authz trigger'ı eksik → WS tembel kapanır → P6-B1'de `rt_authz_cooperatives` trigger'ı ekle.
  5. `roles_name_key` GLOBAL unique (`lower(name)`) → per-coop template klonlama isim çakışmasıyla çöker → `(cooperative_id, lower(name))` rescope gerekir (NULL-coop kendi arasında unique).
- **Doğrudan-DB kısıtı:** `kooperatif_app` `cooperatives`/`users`/`user_sessions`'da grant'siz; yeni yüzeyler `EXECUTE`-grant'li definer + PUBLIC-revoke kalıbıyla.
- **Audit:** mevcut event'ler `cooperative_created/bootstrapped/membership_granted/context_switched` + membership yaşam-döngüsü tetikleri. Eksik: `cooperative_activated/suspended/archived/reactivated`, HTTP `membership_added` atıfı.

## 6. Operasyonel ön koşullar

| Konu | Durum | Engel sınıfı |
|---|---|---|
| Yerel backup/restore drill | v66 PASS (66.7s, tüm satırlar) | Yok |
| WAL/PITR | `archive_mode=off` (dev container); üretim hiç doğrulanmadı | **Üretim engelleyici**, yerel engel değil |
| Audit retention | Politika yok (D3 ertelendi); purge yok | Üretim-öncesi karar |
| İkinci-coop aktivasyon koruması | `business_enabled`+`provisioning` gate aktif; aktivasyon mekanizması yok | P6 kapsamı |
| Migration/regrants bütünlüğü | 66/66 temiz replay, fingerprint `926:2621…` stabil | Yok |
| Üretim DR | NOT VERIFIED | **Üretim engelleyici** |

**Yerel P6 geliştirme için blok yok** — tüm iş disposable DB + test harness üzerinde ilerler. İkinci-kooperatif AKTİVASYONU ve üretim için: DR/PITR kanıtı + retention kararı + aktivasyon runbook'u gereklidir.

## 7. Ürün kararları (onay gerekli)

| # | Karar | Öneri |
|---|---|---|
| **K1** | WS scope kaynağı: (A) yalnız session-türetimli — talimatla uyumlu, tek-coop realtime/oturum; (B) doğrulanmış-header + session fallback — per-tab çok-coop, TenantCtx paritesi | **A** (talimat doğrultusunda): `scope=session.active_cooperative_id`; farklı header→403. Per-tab realtime gereksinimi çıkarsa ayrı milestone'da session-per-tab tasarımı. |
| **K2** | İzin isimleri | `cooperatives.manage` (NULL-scope: platform coop yaşam döngüsü) + `memberships.manage` (scoped: üye ekleme/listeleme). `users.manage` üyelik yazmayı sürdürsün mü? Öneri: hayır — `memberships.manage` ile ayır, `users.manage` geriye uyumlu kalsın (mevcut suspend/end rotaları geçişte ikisini de kabul edebilir, tek deprekatif davranış yok). |
| **K3** | Coop oluşturma/aktivasyon yetkisi | Platform admin HTTP API (`POST /api/platform/cooperatives`, `POST /{id}/activate|suspend|archive`); CLI kalır (ops/DR yolu). Oluşturma `provisioning`, aktivasyon ayrı ve explicit adım. |
| **K4** | İlk admin ataması | Aktivasyon payload'unda opsiyonel `first_admin_user_id` → aynı tx'te membership + `system-administrator` template klonu grant'i (M1-K3 kalıbı). |
| **K5** | `business_enabled` semantiği | Yeni kooperatifler için aktivasyonla birlikte `true` — flag'in asıl amacı legacy-migration gate'iydi; yeni coop "doğuştan migrate". `provisioning` ayrıca ek tohum/yapılandırma adımı tutmak istenirse ayrı karar. |
| **K6** | Rol isim tekilliği | `roles_name_key` → `(cooperative_id, lower(name))` rescope (NULL-coop'lar kendi aralarında unique kalır — `COALESCE` veya iki partial index). Template klonlamanın ön koşulu. |
| **K7** | P6 kapsamı: iş uçları genellemesi dahil mi? | **Evet, ayrı batch (B4)** — roadmap P6 kabulü "T1, T4 paths" non-bootstrap business okuma gerektirir; `BusinessAuth`→`TenantCtx` geçişi mekanik ama geniş. Ürün kararı: tek milestone'da mı, yoksa admin-plane ile P6'yı kapatıp business genellemeyi P7-öncesi ayrı batch'e mi taşıyalım. |

## 8. Önerilen P6 batch'leri (bağımlılık-sıralı)

### P6-B1 — Şema + yetki temeli
- Kapsam: `cooperatives.manage` + `memberships.manage` katalog/satır ekleri; `roles_name_key` rescope; `rt_authz_cooperatives` trigger'ı (status/`business_enabled` değişimi → authz); audit event tipleri (`cooperative_activated` vb.); `app_membership_create` + `app_cooperative_*` sealed definer'ları (postcheck'lerle).
- Yeniden kullanım: `app_rbac_verified_scope`, `app_admin_path_count`, `app_session_*` kalıpları, 0064 emitter'ı.
- Kabul: postcheck fail-closed; `kooperatif_app`'e yeni tablo grant'i YOK; regrants fingerprint değişimi beklenen şekilde.

### P6-B2 — Platform coop yönetim API'si
- `POST /api/platform/cooperatives` (create), `GET /api/platform/cooperatives` (list+filtre), `GET /{id}` (detay: üye sayısı, durum), `POST /{id}/activate` (+optional first admin atomik), `POST /{id}/suspend|reactivate|archive` — NULL-scope `cooperatives.manage`.
- Güvenlik: platform router'a paralel, gate dışı; definer-sade yazma; audit + authz emit; suspended coop'un aktif socket'leri prompt kapanır (B1 trigger'ı).
- Adversarial: coop-scope `cooperatives.manage`'le platform çağrısı→403; activate yarışları; provisioning coop'ta business reddi; suspend'de session davranışı (403 ama session yaşar).

### P6-B3 — Scoped üyelik yönetim API'si
- `POST /api/memberships` (mevcut kullanıcıyı koop'a ekle — davet modeli yok, K-kararı), `GET /api/memberships` (üyeler+roller), mevcut suspend/reactivate/end'e `memberships.manage` uyumu.
- Güvenlik: scope sealed-GUC; hedef kullanıcı `user_id` parametresi; cross-coop üye ekleme imkansız; son-admin guard; audit.
- Adversarial: A-admini B'ye üye ekleyemez; olmayan kullanıcı→404-değil-doğrulama; ended→reactivate akışı; suspend sonrası anında 403 + WS kapanışı.

### P6-B4 — Context genelleştirmesi (K7'ye bağlı)
- `BusinessAuth`→`TenantCtx` migrasyonu: business router'lar + WS + audit scoped lane; `require_business`→`require_in_cooperative`; `me`'nin izin/rol seti aktif-coop-scope'lu (T4.5); WS scope K1 kararıyla.
- Güvenlik: sealed `request_tx` zaten coop-agnostik; header hâlâ doğrulanır, asla güvenilmez; NeverBootstrapped lane korunur.
- Adversarial: T1.x matrisi satırları (A/B header, session-default, forged header), WS non-bootstrap scope + switch-kapanışı, per-coop RLS doğrulaması tüm business uçlarında.
- Risk: **en büyük batch** — mekanik ama 191 referans; her router'da scope-regression testi.

### P6-B5 — Frontend admin + switcher (P7 ile çakışma notu)
- Coop switcher bileşeni, `POST /api/auth/cooperative` bağlantısı, üyelik-yönetim ekranı (mevcut kullanicilar sayfasına ekleme), platform admin coop ekranı, `me` per-coop perms.
- Alternatif: roadmap gereği tamamı P7'ye ertelenebilir — karar.

### P6-B6 — Final denetim (B5 kalıbı)
- T1/T4/T7.4 matris satırları, adversarial tekrar, drill, rapor.

## 9. Karar

**GO — koşullu.** K1–K7 onayları bekleniyor (varsayılan öneriler yukarıda). Yerel geliştirme için hiçbir engel yok; üretim/ikinci-coop aktivasyonu DR-PITR + retention + runbook gerektirir.

**Keşif tamamlandı — kod yazımı için açık onay bekleniyor.**
