# M1-P5 B4 — AUDIT READ API: KEŞİF VE TASARIM

Salt-okunur keşif. Migration, API veya frontend değişikliği YAPILMADI.
Doğrulama tabanı: `kooperatif_b3_replay_v64` (64/64 temiz replay) +
mevcut test veri tabanları.

---

## 1. Envanter

### 1.1 Tablo

`public.security_events` (migration 0002, genişletilmiş 0018+):

| Kolon | Tip | Not |
|---|---|---|
| `id` | uuid PK | sıralama tie-break için kullanılabilir |
| `occurred_at` | timestamptz NOT NULL | `now()` default; index `DESC` |
| `event_type` | text NOT NULL | DB-minted, whitelist zorlanmış |
| `user_id` | uuid → users | aktör (NULL olabilir) |
| `session_id` | uuid | FK'siz; session materyali değil ama iç referans |
| `metadata` | jsonb | olay yükü — **PII ve tutar içeriyor** |
| `cooperative_id` | uuid → cooperatives | NULL = platform/legacy şeridi |

Index'ler: PK(id), `(cooperative_id, occurred_at DESC)`, `occurred_at DESC`, `user_id`.

### 1.2 Yazım şeritleri (altısı da mühürlü/atıflı)

1. `record_tx` → `app_ctx_audit` — zorunlu; sealed request tx içinde
   atomik; caller-mintable whitelist sadece
   `shareholder_family_changed` + `assessments_generated` (0057 §2).
2. `record_platform_session` → `app_session_audit_platform` —
   best-effort; coop **forced NULL**; login success vb.
3. `record_scoped_session` → `app_session_audit_scoped` —
   best-effort; üyelik DB'de yeniden doğrulanır; cross-coop
   attribution reddedilir.
4. `record_login_failed` → `app_audit_login_failed` — NULL atıf,
   hardcoded `login_failed`.
5. `record`/`record_scoped` — direkt INSERT, **owner-lane only** (CLI);
   `kooperatif_app`'in INSERT'i yok.
6. `app_audit_transition()` — 36 AFTER trigger (`*_audit`), DB'nin
   kendi mint'lediği event'ler; coop atıfı sealed GUC'tan.

Sonuç: `security_events`'e yazım yüzeyi kapalı; **okuma yüzeyi hiç
yok** — `kooperatif_app` üzerinde `SELECT=f, INSERT=f` (canlı kanıt),
tablo FORCE RLS + `security_events_owner_lane` (yalnız `kooperatif`).

### 1.3 Event tipi envanteri

~90 tip. Rust `SecurityEventType` enum (`apps/server/src/auth/audit.rs`)
tam liste; DB trigger'ları ek tipler mint'ler
(`period_opened`, `shareholder_family_membership_ended`,
`admin_sessions_revoked` vb. — 0057 §5, 0060).

Platform şeridi (coop=NULL): `login_succeeded`, `login_failed`,
`logout`, `session_revoked`, `sessions_revoked_all_others`,
`user_created/disabled/enabled`, `cooperative_created`,
`cooperative_bootstrapped`, `bootstrap_role_granted`,
`cooperative_membership_granted`, `cooperative_context_switched`.

Scoped şeridi: tüm domain transition event'leri +
`lockout_prevented`, `shareholder_family_changed`,
`assessments_generated`, `role_*`, `membership_*`.

### 1.4 Metadata içerik denetimi (canlı örnekler)

- Platform: `login_failed` → `{username, reason}`; `login_succeeded` →
  `{username}`; `logout` → `{}`. Parola/token/CSRF **hiçbir yerde
  yok** (sözleşme doğrulandı).
- Scoped: `payment_posted` → `{payment_id, payment_number,
  payer_person_id, amount}`; `person_updated` → `{person_id,
  before:{first_name,last_name}, after:{…}}` — **PII**;
  `payment_reversed` → `{payment_id, reason, reversed_allocation_count}`.

Metadata içerikleri meşru audit verisi ama aynı coop'un iş
verisini/PII'sini içerir → response'ta açık allowlist/politika gerekli.

### 1.5 Mevcut yetki/kalıplar

- Katalog: 31 izin (`authz::catalog::ALL`), DB'de `permissions` seed'li;
  konvansiyon `domain.read`/`domain.manage`. `audit.*` yok.
- docs/18 örnek listesinde `audit.view` geçiyor; docs/19: "Audit UI
  read-only, audit export separately permissioned".
- Scoped okuma kalıbı mevcut: `require_in_cooperative` +
  `session_permissions_scoped` (credential-bound definer).
- Platform şeridi kalıbı mevcut: `/api/platform/users/{id}/disable` —
  NULL-scope `users.manage`.
- Pagination konvansiyonu: offset `?page=&page_size=` (parties) — audit
  için yetersiz (append-only, yüksek hacim, sayfa kayması riski) →
  keyset cursor önerilir.
- Retention: **yok**; docs/19 üretim öncesi politika ister, otomatik
  temizlik yasak.

---

## 2. Güvenlik tasarımı

| Gereksinim | Mekanizma |
|---|---|
| Coop admin yalnız kendi coop'u | SECURITY DEFINER reader: session→aktör, coop→aktif üyelik `resolve_membership` eşdeğeri + `audit.read` in-scope; `WHERE cooperative_id = <verified>` asla caller-controlled değil |
| Platform audit ayrı sınır | Ayrı endpoint + NULL-scope `audit.read` (platform yetkisi = cooperative_id IS NULL ataması) — `users.manage`'den bağımsız açık izin |
| Sıradan kullanıcı okuyamaz | `audit.read` katalog izni yoksa 403; anon/üyesiz reddedilir |
| Read-asla-write | Reader fonksiyonu `STABLE` + yalnız SELECT; `security_events` runtime'da zaten SELECT'siz — tek okuma yolu definer olur |
| Cross-coop sızıntı | Definer coop'u DB'de verify eder (GUC/client input yok); platform şeridi scoped şeride karışmaz (NULL coop yalnız platform lane'de) |
| Hassas alanlar | DTO allowlist; `session_id` dışarıda; metadata ayrı karar (aşağıda §4) |

`audit.read` ≠ `users.manage`: ayrı katalog izni (adım: migration seed +
`catalog::ALL` + `PERMISSION_KEYS` — mevcut "controlled catalog change"
prosedürü).

---

## 3. API önerisi

### 3.1 Endpoint'ler

```
GET /api/audit/events          — coop-scoped (TenantCtx; header veya
                                 bootstrap-pinned scope), `audit.read`
                                 in-scope zorunlu
GET /api/platform/audit/events — platform lane; NULL-scope `audit.read`
                                 zorunlu; coop=NULL event'ler +
                                 opsiyonel coop filtresi
```

İkisi de GET, CSRF-origin muaf (safe read), JSON.

### 3.2 Filtreler (hepsi opsiyonel, doğrulanmış)

- `eventType` — bilinen event tipleri whitelist'i (bilinmeyen → 400).
- `actorUserId` — uuid; scoped lane'de varlığı coop'a indirgenir.
- `from` / `to` — timestamptz; `from > to` → 400; pencere üst sınırı
  (örn. 1 yıl) önerilir — unbounded tarama engeli.
- `cursor` — opaque base64 `(occurred_at, id)` keyset; bozuk → 400.
- `pageSize` — 1..=100, default 50.
- Platform lane ek olarak `cooperativeId` filtresi (scoped lane'de yok
  — scope verified'tan gelir).

### 3.3 Cursor ve sıralama

`ORDER BY occurred_at DESC, id DESC`; cursor `(occurred_at < $c.ts)
OR (occurred_at = $c.ts AND id < $c.id)`. Mevcut
`(cooperative_id, occurred_at DESC)` index'i kapsar; `id` tie-break
indexe eklenebilir — `security_events(coop, occurred_at DESC, id DESC)`
index önerisi (mevcut index'i genişletmek yerine yeni index; append-only
migration).

### 3.4 Response DTO (açık allowlist)

```json
{ "items": [{
    "id": "uuid",
    "occurredAt": "…",
    "eventType": "payment_posted",
    "actor": { "userId": "uuid|null", "username": "string|null" },
    "cooperativeId": "uuid|null",   // platform lane'de; scoped'ta scope'a eşit
    "metadata": { … }               // §4 karar noktası
  }],
  "nextCursor": "opaque|null" }
```

**Dışarıda bırakılanlar:** `session_id` (iç referans), herhangi bir
secrets/token (zaten hiç yazılmıyor — doğrulandı). `username`
resolution: `users` join; silinmiş/NULL aktör → `null`.

Hata davranışı: bilinmeyen coop/event → 403/400; iç hata → 500 log'lu;
varlık gizleme (404 yerine coop-scope'ta filtreleme — event'ler
enümerasyon-safe).

### 3.5 Metadata politikası — §5 karar noktası D1

---

## 4. Performans ve doğrulama

- Index: `(cooperative_id, occurred_at DESC, id)` öneri; platform lane
  `(occurred_at DESC)` ile yeterli (coop IS NULL şeridi küçük).
- Sorgu maliyeti: keyset + index → O(pageSize); unbounded toplu dışa
  aktarım için `pageSize` cap zorunlu.
- Retention: yok — karar noktası D3.
- Test kapsamı (adversarial, disposable DB + `kooperatif_app`):
  - iki coop: A admin'i yalnız A event'leri; B sıfır sızıntı;
  - ortak kullanıcı: A'daki event'i B scope'unda görünmez;
  - `audit.read` olmayan üye → 403; anon → 401;
  - platform lane: NULL-scope `audit.read` olmadan 403; coop-scope
    `audit.read` platform lane'e **yetmez**;
  - forge edilmiş `cooperativeId`/`cursor`/`eventType` → 400;
  - direct `kooperatif_app` `SELECT security_events` → hâlâ 42501;
  - cursor stabilitesi: araya insert girse bile tekrar/kayıp yok;
  - `session_id` response'ta yok; metadata allowlist ihlali yok;
  - DEFINER'ın sahipliği/RLS bypass'ı: SECURITY DEFINER + scope
    verify — `EXPLAIN`/`ANALYZE` ile index kullanımı ölçümü.

---

## 5. Karar noktaları (onay gerekenler — öneri ile birlikte)

**D1 — Metadata görünürlüğü.**
Metadata PII/tutar içeriyor (örn. `person_updated.before/after` isimler,
`payment_posted.amount`).
- (a) *Önerilen:* metadata'yı verbatim döndür — `audit.read` tutan bir
  coop admin'i aynı coop'un iş verisine zaten domain read izinleriyle
  erişebilir; audit'in değeri tam yükte. Ancak **scoped lane'de**
  metadata'daki `username`/`reason` gibi platform-yan alanları temizle
  (login event'leri scoped'ta zaten yok).
- (b) Event-tipi başına metadata allowlist — güvenli ama ~90 tip için
  bakım ağır.
- (c) Metadata'yı hiç döndürme — audit değersizleşir.

**D2 — İzin adı.** `audit.read` (katalog `.read` konvansiyonu,
*önerilen*) vs `audit.view` (docs/18 örnek listesi). Tek izin;
`audit.manage`/export ayrı ve bu adımın dışında.

**D3 — Retention.** Şu an yok. *Öneri:* B4 kapsamına alma; docs/19
üretim-öncesi politika kararını ayrı tut. Otomatik temizlik yok.

**D4 — Platform lane kapsamı.** *Öneri:* platform lane yalnız
`cooperative_id IS NULL` event'leri döndürür; coop-scoped olaylara
platform admin erişimi "hangi coop" filtresiyle `cooperativeId`
parametresi vererek açılır mı? *Önerilen:* açılır (platform admin
olay-inceleme görevi) — ama her event'in coop'u açıkça `cooperativeId`
alanında işaretlenir.

**D5 — `session_id` exposure.** *Öneri:* response'ta yok (audit için
aktör yeterli; session FK'sı dışa değer katmaz, saldırı yüzeyini küçültür).

**D6 — Audit ekranında `authz` realtime invalidation.** Yeni audit
sayfası varsa `authz`/domain event'lerinde invalidate — *öneri:* evet,
B3 altyapısı hazır; ama audit listesi ayrı domain tag'i ister mi? —
*Öneri:* hayır, audit okuma page-based refresh yeterli (event'ler
zaten admin paneline düşüyor).

---

## 6. Önerilen B4 implementasyon planı (onay sonrası)

1. Migration `0065`: `permissions` seed'e `audit.read`; yeni index
   `(cooperative_id, occurred_at DESC, id)`; SECURITY DEFINER reader
   `app_audit_read_scoped(token_hash, coop, filters…)` +
   `app_audit_read_platform(token_hash, …)`; PUBLIC revoke; grant
   `kooperatif_app`; postcheck'ler.
2. `authz::catalog::AUDIT_READ` + `ALL` güncellemesi +
   `PERMISSION_KEYS` contracts aynalaması.
3. `apps/server/src/audit/{mod,repo,routes}.rs`: iki handler,
   `TenantCtx`/`BusinessAuth` scope doğrulaması, definer çağrısı,
   DTO allowlist, opaque cursor codec.
4. Frontend: `/denetim` admin sayfası (tr-TR), `audit.read` gate,
   keyset "daha fazla yükle" UX'i.
5. Adversarial test dosyası `tests/audit_read.rs` + realtime
   etkileşimi.
6. Kapılar: realtime + tam backend + frontend + fmt/clippy + temiz
   replay v65 + regrants + drill.

## 7. GO / NO-GO

**GO — koşullu**: tasarım hazır, risk düşük; kod yazımı D1/D2/D4
onaylarından sonra başlar.
