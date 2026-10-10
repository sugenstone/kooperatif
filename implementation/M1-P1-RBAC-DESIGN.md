# M1-P1-ARCH-001 — Kooperatif-Bazlı RBAC Geçiş Tasarımı

Tarih: 2026-10-10 · Baseline: `d652662` (P0 REMOTELY VERIFIED, run 38039772533)
Durum: PLANLAMA — kod/migration yok. P1 implementasyonu sahip onayı bekler.

Etiketler: **[VERIFIED]** doğrulanmış bulgu · **[DESIGN]** tasarım önerisi ·
**[OWNER]** sahip kararı gerekir · **[RISK]** çözülmemiş risk

---

## 1. Mevcut RBAC Envanteri [VERIFIED]

### 1.1 Şema (migration 0003)

| Tablo | Kapsam | Kritik kısıtlar |
|---|---|---|
| `permissions` | GLOBAL katalog (31 anahtar, sadece migration tohumlar) | `key` unique; runtime API yok |
| `roles` | **GLOBAL** | `lower(name)` unique global; `status` active/disabled |
| `role_permissions` | role→permission | PK `(role_id, permission_id)` |
| `user_role_assignments` | user→role | PK `(user_id, role_id)` |
| `cooperative_memberships` | coop×user (P0) | `UNIQUE(coop,user)`; status active/suspended/ended; satır silinmez |

Tohumlu rol: `Sistem Yöneticisi` (`00000000-0000-4000-8000-000000000001`) —
yalnızca 4 yönetim izni (users/roles read+manage), hiçbir yerde
special-case değil; kuvveti = izin seti.

### 1.2 Hesaplama / uygulama noktaları

| Konum | İşlev | Kapsam |
|---|---|---|
| `authz::effective_permissions(pool,user)` | izin birleşimi | GLOBAL — coop yok |
| `authz::require(state,auth,perm)` | her istekte taze DB çözümü | GLOBAL; ~90 çağrı noktası (15 routes dosyası) |
| `authz::administration_path_count(tx)` | son-yönetici kilidi | GLOBAL sayım |
| `authz::begin_guarded_transaction` | advisory lock `0x4B4F4F50303033` | GLOBAL tek anahtar |
| `auth/routes.rs` `authorization_context()` | `me`+login'e `permissions`,`roles` yazar | GLOBAL |
| `rbac.rs` (8 route) | rol/üye yönetimi | GLOBAL listeler, GLOBAL yazma |
| `main.rs` `grant-role` | CLI rol atama | GLOBAL rol adı, üyelik kontrolü yok |
| Frontend `auth.permissions` + `can()` | UX ipucu | GLOBAL |
| `tenant/extractor.rs` | `TenantCtx` — coop doğrulama | coop-bazlı (P0) |

**Tespit edilen tek yetki sızıntısı kalan yüzey:** `require` global izin
setine bakar; P0-002 kapısı bunu kaba bir kill-switch'e indirgemişti.
P1, `require`'ı coop-bazlı hale getirerek kapıyı "var/yok"tan "hangi
yetki" seviyesine yükseltir.

---

## 2. Hedef Yetkilendirme Modeli [DESIGN]

Invariantlar (onaylı M1-K5 + bu görev §2):

1. Rol `cooperatives`'e aittir; global rol kavramı yoktur.
2. Atama `(cooperative_id, user_id, role_id)` — DB kompozit FK ile
   üyelik ve aynı-coop rolü zorunlu kılar: **cross-coop atama DB'de
   imkânsız** (7.15 FK-saldırısı senaryosu).
3. `effective_permissions(user, coop)` tek doğrulanmış context'te
   hesaplanır ve aktif üyelik şartı içerir:
   `m.status='active'` koşulu UNION içine gömülür → suspended/ended
   üyelik anında sıfır yetki (ekstra adım gerekmez).
4. Frontend `permissions` UX ipucu; sunucu her istekte taze hesaplar
   (ADR-002 — cookie'de yetki yok, değişmiyor).
5. Eksik/geçersiz context → global yönetici mirası **asla** (P0-003
   fail-closed kuralı korunur).

### 2.1 Yetki çözümleme sorgusu (hedef)

```sql
SELECT DISTINCT p.key
FROM user_role_assignments ura
JOIN roles r ON r.id = ura.role_id AND r.status = 'active'
JOIN role_permissions rp ON rp.role_id = r.id
JOIN permissions p ON p.id = rp.permission_id
JOIN cooperative_memberships m
  ON m.cooperative_id = ura.cooperative_id
 AND m.user_id = ura.user_id
 AND m.status = 'active'
WHERE ura.user_id = $1 AND ura.cooperative_id = $2;
```

Üyelik JOIN'i sayesinde askıya alınan üyelik aynı transaction'da değilse
bile sonraki istekte sıfır yetki üretir — revocation anlık.

---

## 3. Migration Tasarımı [DESIGN] + [OWNER-K1]

### 3.1 Alternatif karşılaştırması

| | A — mevcut tablolara `cooperative_id` | B — yeni `cooperative_*` tablolar |
|---|---|---|
| Kod yolu | tek | çift (eski+yeni okuyucu) |
| Veri korunması | in-place backfill | kopya + eski tablo ölü ağırlık |
| Rollback | `cooperative_id` nullable → kolay; NOT NULL sonrası yeni migration ile geri alınamaz ama backup drill kapsar | tablo drop — kolay |
| Uyumluluk | `role_permissions` değişmez (rol coop'undan miras) | üç yeni tablo + FK ağı |
| Karmaşıklık | düşük | orta-yüksek |

**Öneri: A.** M1-K5 "global rol yok" dediği için eski tabloların
yaşamaya devam edeceği meşru bir kullanım yok; çift model gereksiz
saldırı yüzeyi üretir.

### 3.2 Migration 0019 (önerilen sıra, tek transaction)

```sql
-- expand
ALTER TABLE roles ADD cooperative_id UUID REFERENCES cooperatives(id);
ALTER TABLE user_role_assignments ADD cooperative_id UUID REFERENCES cooperatives(id);

-- backfill (tek bootstrap coop garantili — partial unique)
UPDATE roles SET cooperative_id = (SELECT id FROM cooperatives WHERE is_bootstrap);
UPDATE user_role_assignments ura SET cooperative_id =
    (SELECT r.cooperative_id FROM roles r WHERE r.id = ura.role_id);

-- DO $$ assert: null kalan satır yok; admin_path(bootstrap) >= 1 $$

-- contract
ALTER TABLE roles ALTER COLUMN cooperative_id SET NOT NULL;
ALTER TABLE user_role_assignments ALTER COLUMN cooperative_id SET NOT NULL;
DROP INDEX roles_name_key;                       -- global unique kalkar
CREATE UNIQUE INDEX roles_name_uq ON roles (cooperative_id, lower(name));
CREATE UNIQUE INDEX roles_coop_id_uq ON roles (cooperative_id, id);
ALTER TABLE user_role_assignments
  ADD FOREIGN KEY (cooperative_id, user_id)
      REFERENCES cooperative_memberships (cooperative_id, user_id),
  ADD FOREIGN KEY (cooperative_id, role_id)
      REFERENCES roles (cooperative_id, id);
```

`role_permissions` ve `permissions` **değişmez**: katalog global kalır
(anahtar namespace uygulamaya ait; her coop aynı katalogdan seçer —
aynı `payments.manage` anahtarı A'da da B'de de ayrı rolle atanır).

### 3.3 `is_template` ile şablon rol (M1-K3)

`roles`'a `is_template BOOLEAN NOT NULL DEFAULT false` — bootstrap'taki
tohum `Sistem Yöneticisi` (ve ileride tanımlanacak onaylı şablonlar)
işaretlenir; `create-cooperative` provisioning şablonları hedef coop'a
**kopyalar** (yeni id, aynı izin seti). Klon sonrası bağımsız —
değişiklikler yayılmaz (M1-K3).

### 3.4 Son-yönetici kilidi coop-bazlı

- `administration_path_count(tx, cooperative_id)` → `JOIN cooperative_memberships m … AND ura.cooperative_id=$2 AND m.status='active' AND u.status='active'`.
- Advisory lock: tek global anahtar yerine iki-int formu
  `pg_advisory_xact_lock(NAMESPACE, coop_i32)` — `coop_i32` = uuid'nin
  ilk 32 biti; çakışma yalnızca fazladan serileştirir (yanlış izin
  üretmez). **[DESIGN]**; alternatif: global kilidi koru (basit, trafik
  az) — performans gerekene dek önerilmez.

---

## 4. Bootstrap Yönetici Geçişi

- Mevcut roller + atamalar backfill ile **bootstrap coop**'a taşınır —
  `Sistem Yöneticisi` sahibi yönetim erişimini korur; gelecek
  kooperatiflere **otomatik geçmez** (atama (coop,user,role) bağlı,
  yeni coop'ta satır yok).
- Kilitlenme önlemi: migration `DO` bloğu backfill sonrası
  `administration_path_count(bootstrap) >= 1` assert eder; sağlanmazsa
  transaction rollback → schema değişmez → backup drill prosedürüyle
  teşhis/restore.
- Yeni coop yöneticisi: `create-cooperative --admin <username>`
  (tek transaction: üyelik + şablon rol klonu + atama) — aksi halde
  `add-member` + `grant-role --cooperative` iki adımı.
- `grant-role --username U --role R --cooperative K`: rol coop adıyla
  çözülür, kullanıcının o coop'ta aktif üyeliği şart; `--cooperative`
  verilmezse bootstrap (geriye uyum). Audit `cooperative_id` ile.

---

## 5. API / Frontend Sözleşmesi [DESIGN] + [OWNER-K2]

- `me` / login yanıtı: `cooperatives[]` her üyeliğe `permissions[]` +
  `roles[]` ekler (coop başına hesaplanır). Üst-seviye `permissions`
  + `roles` P1'de **bootstrap-scoped** değer taşımaya devam eder
  (legacy uyum — P7'de kaldırılır). Session varsayılanı açık header'ı
  asla geçersiz kılamaz (zaten böyle — dokümante edilir).
- rbac endpoint'leri (8 route): P1'de sorgular **bootstrap coop**'a
  bağlı (`legacy authorization scope` — §6); roller coop-scoped
  oluşur/listelenir. `users/{id}/roles` yalnız context coop'un
  rollerini kabul eder — FK zaten zorlar.
- `GET /api/permissions`: global katalog — coop'tan bağımsız, değişmez.
- CLI: `grant-role --cooperative`; `create-cooperative --admin`;
  `add-member` mevcut.
- Sekmeler: header istek-bazlı → tab-A(A coop) ile tab-B(B coop)
  ayrı izin setleri; `me` yenilemesi sekmeye özel header ile doğru
  context'e hizmet eder.

## 6. Legacy Uyumluluk (kritik)

P2'ye dek iş verisi coop'suzdur → legacy endpoint'lerin yetki context'i
tek olası doğru değere sabitlenir: **`legacy_scope() = is_bootstrap
cooperative`**.

- Header'sız istek → `effective_permissions(user, bootstrap)` + aktif
  bootstrap üyeliği şartı. B-only üye: gate (enabled üyelik yok) zaten
  403; B üyesi A üyesi olmayan: bootstrap üyeliği yok → yetki boş →
  `permission_denied`. Tutarlı.
- Açık header: `TenantCtx`/`gate` business_enabled şartı → yalnız
  bootstrap geçebilir → `require` yine bootstrap'ta değerlendirir;
  **enabled-olmayan context asla yetki üretmez**.
- P0 `business_membership_gate` **korunur** — P1 kapıyı kaldırmaz,
  granüler hale getirir: "enabled üyelik var mı" → "bu coop'ta bu izin
  var mı". Kapı P2'ye kadar yedek savunma olarak kalır.
- İkinci coop `business_enabled=false` kaldıkça business endpoint'lere
  ulaşamaz — P1 bunu değiştirmez.

## 7. Güvenlik Test Matrisi

DB (repo): 7.1 coop-scoped `effective_permissions` birleşimi; 7.2 FK
cross-coop atama reddi; 7.3 suspended üyelik → boş set; 7.4 migration
backfill + admin-path assert.
HTTP: 7.5 A-admin/B-viewer → B header'ı B perms (P2 öncesi B enabled
olamayacağından: context kurulamaz → 403); 7.6 headerless dual-member →
bootstrap perms; 7.7 sıfır üyelik → 403 (P0-003 regresyon); 7.8 revoked
üyelik → sonraki istekte yetki sıfır; 7.9 global-rol eskalasyonu
(B'de hiç rolü olmayan A-admin → bootstrap yetkisi vardır ama B context'i
kurulamaz); 7.10 son-yönetici kilidi coop-bazlı (A admin'i B'deki admin
yolunu etkilemez); 7.11 eşzamanlı istekler farklı context; 7.12 aktif
oturumda rol değişikliği → sonraki istekte etkili (cache yok);
7.13 CLI `grant-role --cooperative` audit'li + üyelik şartı;
7.14 audit'te `cooperative_id` attribution; 7.15 composite-FK saldırısı
(ura'ya yabancı coop id enjekte → FK reddi); 7.16 `set_user_roles`
başka coop rol id'si → red.
E2E (P7 öncesi n/a): sekme-bağımsız context + coop switcher UI — P8
matrisine ertelenir.

## 8. P1 Uygulama Fazları

| Faz | Kapsam | Dosyalar | Test | Kabul | Rollback |
|---|---|---|---|---|---|
| P1-a | migration 0019 (schema+backfill+assert) | `migrations/0019_*` | DB: FK, backfill, lockout | schema+veri tutarlı | tek tx; backup drill |
| P1-b | coop-scoped authz çekirdeği | `authz.rs` | unit+DB: 7.1–7.4 | `require` bootstrap-scope | kod geri alınabilir |
| P1-c | rbac endpoint'leri bootstrap-scope | `rbac.rs`, audit | HTTP: rol CRUD scope | B rolü görünmez | geri alınabilir |
| P1-d | CLI `--cooperative` + `--admin` + şablon klonu | `main.rs`, `tenant/repo.rs` | DB: provisioning | yeni coop admin izole | idempotent CLI |
| P1-e | `me` per-coop perms + frontend | `auth/routes.rs`, contracts, `auth.svelte.ts` | HTTP+vitest | sözleşme dokümante | additive alanlar |
| P1-f | matris + regresyon + doküman | test dosyaları, registry | tam suite | tüm gate'ler | — |

Bağımlılık: hepsi P0'a dayanır; P1-a → b → c/e → d → f.

## 9. Sahip Kararları [OWNER]

- **P1-K1** (§3.1): Tablo yaklaşımı A (extend) öneriliyor — B istenirse
  tasarım yeniden yapılır.
- **P1-K2** (§5): `me`'de per-coop `permissions`/`roles` + üst-seviye
  alanların bootstrap-compat korunması öneriliyor.
- **P1-K3** (§6): Legacy yetki context'inin bootstrap-coop'a
  sabitlenmesi onayı — P2'ye dek tek doğru semantik.
- **P1-K4** (§3.3): `is_template` şablon-rol mekanizması + ilk şablon =
  tohum `Sistem Yöneticisi`.
- **P1-K5** (§3.4): Per-coop advisory lock (önerilen) vs global lock
  koruma (daha basit).

## Açık Riskler [RISK]

- P1 ile P2 arası geçiş dönemi: bootstrap'a bağlı yetki — B coop ancak
  P2 sonrası business_enabled olabilir; ara adımda coop-B admin'i
  kendi rollerini yönetemez (rbac endpoint'leri bootstrap'a kilitli).
  Çözüm P6 (admin API) veya P1-c'yi TenantCtx'e genişletmek —
  **[OWNER-K3]** ile ilişkili; öneri: P1-c'yi bootstrap-scope tutup
  coop-B admin UI'ını P7'ye ertelemek (B zaten business_enabled=false).
- `user_role_assignments`'ta tarihsel atama kaydı yok (satır
  create/delete — audit event'leriyle telafi; değişmez).
