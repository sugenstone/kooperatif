# M1-P1-003 — P1-c Authorization Integration Safety Design

Durum: **PLANLAMA — uygulama başlatılmadı.** Baseline: `977e33d`
(P1-b). P0 `d652662` uzaktan doğrulandı; 0019 şeması yerinde; scoped
resolver (`effective_permissions_in_cooperative`,
`require_in_cooperative(&TenantCtx)`) mevcut ve 7/7 testli.

## 1. Authorization Çağrı-Envanteri (tam)

| # | Grup | Dosya | `require` sayısı | Mevcut mekanizma | Gelecek context | Göç fazı |
|---|------|-------|---|---|---|---|
| 1 | Finansal/iş endpoint'leri | `reports/routes.rs` | 16 | global `require` + CurrentAuth | bootstrap-pinned `BusinessAuth` | P1-c3 (grup 5) |
| 2 | Finansal/iş | `payments/routes.rs` | 9 | aynı | aynı | P1-c3 (grup 2) |
| 3 | Finansal/iş | `financial_accounts/routes.rs` | 9 | aynı | aynı | P1-c3 (grup 2) |
| 4 | Finansal/iş | `social_aid/routes.rs` | 8 | aynı | aynı | P1-c3 (grup 4) |
| 5 | Finansal/iş | `parties/routes.rs` | 7 | aynı | aynı | P1-c3 (grup 1) |
| 6 | Finansal/iş | `periods/routes.rs` | 6 | aynı | aynı | P1-c3 (grup 2) |
| 7 | Finansal/iş | `income_expense/routes.rs` | 6 | aynı | aynı | P1-c3 (grup 3) |
| 8 | Finansal/iş | `governance/routes.rs` | 6 | aynı | aynı | P1-c3 (grup 4) |
| 9 | Finansal/iş | `shares/routes.rs` | 4 | aynı | aynı | P1-c3 (grup 1) |
| 10 | Finansal/iş | `share_returns/routes.rs` | 4 | aynı | aynı | P1-c3 (grup 3) |
| 11 | Finansal/iş | `credits/routes.rs` | 4 | aynı | aynı | P1-c3 (grup 3) |
| 12 | Finansal/iş | `investments/routes.rs` | 3 | aynı | aynı | P1-c3 (grup 4) |
| 13 | RBAC yönetimi | `auth/rbac.rs` | 15 (7 doğrudan + 8 `guarded_mutation!`) | global `require` + global rol sorguları | scoped sorgu + per-coop lockout | **P1-c2** |
| 14 | Auth sözleşmesi | `auth/routes.rs` (`me`) | 0 (`effective_permissions` + `active_role_summaries`) | global resolver | per-coop alanlar | P1-e |
| 15 | Realtime | `realtime/routes.rs` | 0 (`effective_permissions` `revalidate` içinde) | global resolver / WS sinyal kapsamı | scoped resolver (bootstrap) | P1-c3 (grup 5) veya P5'e erteleme |
| 16 | CLI | `main.rs grant-role` | — | NULL-coop INSERT | `--cooperative` zorunlu | **P1-d (c4 önkoşulu)** |
| 17 | Tenant endpoints | `tenant/routes.rs` | 0 | TenantCtx | — | değişmez |

Toplam: **82 iş-endpoint `require` + 15 RBAC + 3 resolver noktası +
1 CLI yazıcı.**

NULL-rol/atama uyumluluk şeridi: 0019 sonrası backfill'lenmemiş satırlar
(bootstrap öncesi DB'ler, post-bootstrap `grant-role` yazımları) NULL
kalır; bunlar "bootstrap mantıksal sahibi" olarak yorumlanır — sorgu
deseni §4.

## 2. Tek Doğrulanmış Authorization Context — `BusinessAuth`

Yeni extractor (`tenant::extractor` veya `authz` yanında), `CurrentAuth`
üzerine:

```text
resolve():
  header x-cooperative-id varsa:
      UUID parse başarısız          → 400 CooperativeContextRequired
      id != bootstrap.id            → 403 CooperativeAccessDenied
      (üye varsa bile; explicit non-bootstrap ASLA sessiz bootstrap'a
       düşmez)
  header yoksa:
      resolved := bootstrap.id      (session default YOKSAYILIR —
      legacy endpoint'ler için anlamsız ve karışıklık kaynağı)
  bootstrap yoksa                   → ctx.cooperative_id = None
      (pre-tenant legacy şeridi — P0 gate'in aynı istisnası)

### Bootstrap çözümü — dayanıklı marker (M1-P1-005, uygulandı)
`is_bootstrap` bayrağı tek başına delil değildir: flag-stripped ya da
silinmiş bootstrap satırı + kayıp audit izi, global şeridi yanlışlıkla
yeniden açabilirdi. `0020_installation_state.sql` ile tek satırlı
`installation_state` tablosu eklendi; `bootstrap_cooperative_id` FK'si
bootstrap kooperatifini referanslı iken SİLİNEMEZ kılar ve marker yalnızca
`bootstrap_initial_cooperative` tarafından, kooperatifle AYNI transaction
içinde yazılır — sıradan kooperatif yönetimiyle kaldırılamaz.
`resolve_bootstrap()` marker'dan JOIN'ler ve `c.is_bootstrap` tutarlılığı
şarttır; tutarsızlık (flag-strip, non-bootstrap hedef) → çözüm yok →
`never_bootstrapped` residue kontrolüyle fail-closed. `never_bootstrapped`
artık 4 kanıt ister: singleton marker satırı mevcut + marker NULL +
sıfır kooperatif + sıfır coop-attributed audit. Yükseltme yolu: 0020,
mevcut bootstrap kurulumları için marker'ı geriye dönük doldurur.
  sonrasında resolve_membership() ile TenantCtx ile BİREBİR aynı
  doğrulama: membership yoksa / status != 'active' → 403;
  coop status != 'active' → 403; business_enabled=false →
  CooperativeNotReady
```

`BusinessAuth { auth, cooperative_id: Option<Uuid>, cooperative_name }`.

Yetki kontrolü tek fonksiyon:

```rust
authz::require_business(state, &ctx: &BusinessAuth, perm)
  ctx.cooperative_id = Some(b) → effective_permissions_in_cooperative
  ctx.cooperative_id = None    → legacy global effective_permissions
                                (yalnızca pre-tenant DB'de mümkün)
```

Neden extractor ve yardımcı fonksiyon (middleware'e gömme değil): P0
`business_membership_gate` route_layer olarak kalır — defense-in-depth;
`BusinessAuth` her handler'da context'i tip-güvenli hale getirir ve
`TenantCtx`'ten bootstrap-pinning ile ayrışır (session default'unu
yoksayması ve explicit-non-bootstrap'ı reddetmesi TenantCtx'te yok).
Ham UUID'den context İNŞA EDİLMEZ — her zaman `resolve_membership`
sonucu doğrulanır.

P1-K3 uyumu: headerless → yalnızca bootstrap; explicit non-bootstrap →
403; üyelik iptali → membership JOIN ile anında 403/boş küme; global
fallback yalnızca `cooperatives` tamamen boşken (pre-tenant).

## 3. P1-c Alt Fazları (bağımsız testli)

### P1-c1 — Authorization Context Adapter
- `BusinessAuth` extractor + `require_business` + hata mapping'i.
- Endpoint'e DOKUNULMAZ; yalnızca birim/DB/HTTP seviyesinde test.
- Testler: headerless→bootstrap; explicit bootstrap→OK; explicit
  non-bootstrap→403 (üye olsa bile); malformed→400; bilinmeyen UUID→403;
  suspended→403; bootstrap-öncesi (coop yok)→legacy global require
  davranışı; session-default B iken headerless→yine bootstrap.

### P1-c2 — RBAC Management API Scoping
- `rbac.rs` 15 `require` → `require_business` + `BusinessAuth`.
- Rol listeleme/detay sorguları: `WHERE (cooperative_id = $c OR
  (cooperative_id IS NULL AND $c = bootstrap))` — NULL satırların
  bootstrap'a mantıksal atfedilmesi.
- Rol oluşturma INSERT'leri artık `cooperative_id = ctx` yazar
  (yeni NULL yazıcı bırakmaz — c4 önkoşulu).
- Rol atama/kaldırma sorguları scoped `ura` yazar/günceller; hedef
  kullanıcının ctx üyeliği zorunlu (FK zaten garanti, uygulama da
  doğrular — kullanıcı-dostu hata için).
- `administration_path_count(tx, coop)` → scoped varyant
  (`administration_path_count_in_cooperative`): aynı NULL→bootstrap
  görünürlük kuralıyla; global sürüm korunur (P1-K5 geçişi).
- Advisory lock: `pg_advisory_xact_lock(0x4B4F4F50, hashtext(coop::text))`
  — coop-bazlı anahtar.
- Testler: bootstrap rol listesi yalnızca bootstrap satırları (+NULL
  legacy); B-coop rolü görünmez/erişilemez/atanamaz; cross-coop rol id
  ile PUT → 403/404; son-yönetici koruması coop sayımıyla;
  eşzamanlı admin mutasyonları.

### P1-c3 — Business Authorization Migration (router grupları)
Her grup ayrı commit/test: handlers `CurrentAuth`→`BusinessAuth`,
`require`→`require_business`.

| Grup | Dosyalar | require |
|---|---|---|
| 1 | parties + shares | 11 |
| 2 | payments + financial_accounts + periods | 24 |
| 3 | income_expense + credits + share_returns | 14 |
| 4 | social_aid + governance + investments | 17 |
| 5 | reports + realtime | 16 + resolver |

- Her grup için doğrudan HTTP testi: bootstrap üyesi yetkili; NULL-coop
  legacy atamalı kullanıcı (bootstrap mantıksal) yetkili; askıya alınmış
  üye 403; explicit B header → 403; B-only üye → 403.
- Realtime `revalidate`: `effective_permissions` → bootstrap pinned ise
  scoped, bootstrap yoksa global (BusinessAuth ile aynı dallanma).

### P1-d — (bağımlılık revizyonu) `grant-role --cooperative`
**C4 ÖN KOŞULU.** NULL yazan son aktör CLI'dir; `--cooperative`
zorunlu kılınmadan NOT NULL kontratı yapılamaz. P1-d'nin CLI yarısı
öne çekilir; şablon-klonlama yarısı ayrı kalabilir.

### P1-c4 — Contract Migration (0020)
Ön koşul: c2 + c3 + d-CLI → hiçbir NULL yazıcı kalmadı.
- Backfill replay (0019 deseni): roles → bootstrap (koşulsuz); ura →
  yalnızca üyeliği olanlar; kalan NULL ura VARSA → `RAISE EXCEPTION`
  (fail-loud; operatör `add-member` veya atamayı siler, tekrar dener —
  sessiz üyelik yaratma veya rol silme YOK).
- `SET NOT NULL` her iki sütunda.
- `roles_name_key` → `UNIQUE (cooperative_id, lower(name))` swap.
- Assert: bootstrap admin-path ≥1 (0019 deseni).

### P1-c5 — Audit + Regresyon
- `security_events.cooperative_id` attribution'ı rol mutasyonlarına.
- Çağrı-envanteri kapanış kanıtı (grep: sıfır `authz::require(` iş
  yollarında).
- Tam finansal regresyon + backup drill + admin-süreklilik doğrulaması.

Bağımlılık zinciri: c1 → c2 → c3 → **d-CLI** → c4 → c5. (P1-e ve P1-f
önceki plana paralel: e, c2 sonrası herhangi bir noktada olabilir.)

## 4. Migration Güvenliği

| Konu | Analiz |
|---|---|
| NULL-coop satırlar | c2/c3 sorgu deseni `(coop = ctx OR (NULL AND ctx = bootstrap))` ile bootstrap'a atfeder; c4 fail-loud kapanır |
| Bootstrap backfill | 0019 bootstrap sonrası oluşan satırları kapsamaz → c2'den itibaren tüm yazıcılar coop yazar; c4 replay straggler'ları yakalar |
| `grant-role` NULL yazar | **c4 bloklayıcısı** → d-CLI öne çekildi |
| Global `me` sözleşmesi | c4'ü bloklamaz; P1-e'de scoped alanlara geçer |
| Yeni coop rol provisioning | `system_role_templates` (0019) hazır; klonlama P1-d'nin ikinci yarısı — c4'ten BAĞIMSIZ |
| `roles_name_key` | Global unique `(lower(name))` — c4'te `(coop, lower(name))` olur; NULL kalmadığı için ara durum gerekmez |
| FK/işlem sırası | 0019'un kompozit FK'ları zaten yerinde; c4 yalnızca NOT NULL + indeks swap'ı — tek transaction, assert'li |
| `ura` PK `(user_id, role_id)` | Rol tek coop'a ait → PK yeterli; değişmez |

## 5. Güvenlik Test Matrisi (hedef)

| Senaryo | Katman | Beklenen |
|---|---|---|
| A-admin / B-viewer | DB+HTTP | A'da yönetir; B kapsamında boş küme |
| A-viewer / B-admin | DB+HTTP | simetrik izolasyon |
| B üyeliği yok | HTTP | iş endpoint 403 |
| suspended / ended üyelik | HTTP+DB | anında 403 |
| Geçersiz header (malformed) | HTTP | 400 |
| Bilinmeyen UUID header | HTTP | 403 |
| Explicit non-bootstrap header | HTTP | 403 (üye bile olsa) |
| Headerless | HTTP | yalnızca bootstrap yetkisi |
| Cross-coop rol ataması | DB | FK reddi (0019'da kanıtlı) |
| Son yönetici kaldırma | HTTP | LockoutPrevented (coop sayımı) |
| Eşzamanlı admin mutasyonları | HTTP | advisory lock tek galip |
| NULL legacy rol/atama | DB+HTTP | bootstrap'ta çalışır, scoped sızıntı yok |
| Rol iptali aktif oturumda | HTTP | sonraki istek 403 (cache yok) |
| Çok-sekme context | E2E (P1-f) | sekme-bağımsız header |
| Audit coop attribution | DB | `security_events.cooperative_id` dolu |

## 6. Kalan Riskler

- `BusinessAuth` + gate çift doğrulaması gereksiz görünebilir ama
  katman ayrımı korunuyor (gate = erişim kapısı; extractor = context).
- `me` global kalana dek UI'da B-coop için yanıltıcı izin listesi
  görünebilir — yalnızca UX ipucu, sunucu otorite (P1-e kapatır).
- Realtime resolver'ın bootstrap-pin'i P5 kapsamındaki tam
  coop-izolasyonunun yerine geçmez.
- c4 fail-loud tasarımı canlı sistemde operatör müdahalesi gerektirebilir
  — kurtarma prosedürü dokümante (0019 deseni).

## 7. Önerilen İlk Adım

**P1-c1** — `BusinessAuth` + `require_business` + izolasyon testleri.
Tek dosya, endpoint değişikliği yok, tamamen geri alınabilir.

## 8. P1-c1 uygulama durumu (M1-P1-004)

TAMAMLANDI ve yerel commit'lendi:

- `tenant/extractor.rs`: `BusinessScope` (`Bootstrap` |
  `NeverBootstrapped`) + `BusinessAuth` extractor; üyelik doğrulama
  `verify_business_membership` helper'ına çıkarıldı (TenantCtx ile
  paylaşılan tek doğrulama).
- `tenant/repo.rs`: `never_bootstrapped` — sıfır kooperatif AND sıfır
  coop-attributed `security_events`; eksik bootstrap satırı + residue →
  fail-closed.
- `authz.rs`: `effective_permissions_in_bootstrap` (NULL şeridi YALNIZCA
  bootstrap'a atfedilir — deterministic verified attribution) +
  `require_business(state, &BusinessAuth, perm)`.
- `tests/business_auth.rs`: probe router üzerinden 8/8 — explicit
  bootstrap, headerless pin + session-default yoksayma, explicit
  non-bootstrap/unknown/malformed red, suspended/ended/sıfır-üyelik,
  business_enabled=false bootstrap, never-bootstrapped legacy şeridi,
  residue fail-closed, NULL legacy attribution (bootstrap'ta çalışır /
  B-only üyeye sızmaz), eşzamanlı context bağımsızlığı.
- Endpoint'lere BAĞLANMADI (P1-c3); mevcut davranış değişmedi.
