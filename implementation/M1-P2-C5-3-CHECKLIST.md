# M1-P2-C5-3 — EXECUTION CHECKLIST (hazırlık, icra edilmedi)

**Durum:** Salt-okunur hazırlık. C5-3 tek transaction'lık "constraint swap + NULL-lane kapanışı"dır.
**Ön-şart:** C5-2 deploy'u tamamlanmış ve tüm ortamlarda 0024 scoped indeksleri + scoped `ON CONFLICT` kodu birlikte aktif olmalı (§0).

## 0. Ön-kapılar (hepsi PASS olmadan başlama)

- [ ] `cargo run -- migrate` her hedef ortamda 0024'ü uygulamış; `_sqlx_migrations` = 24+.
- [ ] `pg_indexes`'te 42 scoped indeks (`*_coop_*_uq`) mevcut ve geçerli.
- [ ] Katalog doğrulaması: `SELECT count(*) FROM pg_constraint c JOIN pg_index i ON i.indexrelid = c.conindid WHERE i.indpred IS NOT NULL AND c.contype = 'u'` — partial unique constraint **yok** (C5-2'nin scoped `ON CONFLICT`'i 0024 partial index'lerini kullanır; constraint'e ihtiyaç yok).
- [ ] **RLS / restore ön-kapısı (ADR-013):** C5-3 global unique'leri düşürür — geri alma penceresi daralır. Restore-verified backup artifact'ı mevcut ve tarihli olmalı.

## 1. Tek transaction içinde yapılacaklar (0025 taslağı)

### 1a. Drop — 61 eski global unique constraint/index

- 21 numaralama: `*_number_key` + `families_sequence_number_key`
- 20 idempotency: `*_idempotency_key_key` (`credit_applications` dahil — partial index)
- 19 yapısal + `financial_categories_name_key`: **dikkat** — bunların 19'u global kalır kararı C5 tasarımında; drop listesi yalnızca rescope kararı verilenler:
  - DROP edilecekler: 21 numbering + 20 idempotency + `financial_categories_name_key` = **42 constraint**
  - **KALACAKLAR (global):** 19 structural/partial (movement-binding `*_movement_key`, `account_movements_source_leg_key`, `assessments_period_shareholder_key`, `share_returns_one_pending_per_share`, `governance_memberships_active_key`, `cooperatives_name_key`, `users_username_key`, `parties` identity unique'leri vb. — tam liste `M1-P2-C5-DESIGN.md` §envanter)

> Bağımlılık notu: 0024 scoped indeksleri zaten mevcut olduğu için drop anında tekilik koruması kesintisiz sürer — dual-enforcement'tan scoped-only'ye geçiş atomic.

### 1b. NULL-lane kapanışı — legacy `cooperative_id IS NULL` satırları

- `WHERE cooperative_id IS NULL` taraması 37 business tabloda: NULL satır kalmışsa **fail-closed** (`DO $$` guard → exception → rollback). Bootstrap tek kooperatif claim'i sonrası NULL beklenmez.
- NULL lane'i koruyan mekanizma artık global unique değil; NULL satırlar composite unique'te zaten birbirini görmez (NULL != NULL). NULL'ların birbirleriyle çakışmasını engellemek için **20 partial index:** `UNIQUE (idempotency_key) WHERE cooperative_id IS NULL AND idempotency_key IS NOT NULL` — yalnızca NULL-lane uyumluluğu M1-P3'e kadar korunacaksa eklenir; NULL satırı yoksa no-op.
- Karar kapısı: bootstrap ortamında NULL satır = 0 ise partial NULL-lane index'leri **atlanabilir** (daha temiz); >0 ise ekle ve M1-P3'te retirement'a taşı.

### 1c. NOT NULL validasyonu (ayrı transaction önerilir — kilit süresi)

- 37 business tabloda `cooperative_id SET NOT NULL` — **önce** `VALIDATE`-edilmiş `CHECK (cooperative_id IS NOT NULL)` → sonra `SET NOT NULL` deseni (uzun AccessExclusive kilit yerine validasyonlu geçiş). Kanıt: `pg_attribute.attnotnull`.
- **Karar:** C5-3'te NOT NULL **dahil mi** — kullanıcı onayı gerekir (lock riski). Alternatif: M1-P3'e ertele.

## 2. Uygulama-kodu temizliği (C5-3 ile aynı release)

- [ ] 14 `ON CONFLICT (cooperative_id, idempotency_key) WHERE …` sitesi — partial predicate'li `ON CONFLICT` hedefi **geçerli kalır** (0024 index'i hâlâ duruyor); kod değişikliği gerekmez.
- [ ] 5 violation→replay yolu — scoped replay `WHERE cooperative_id = $n` zaten ekli; C5-3 sonrası global constraint kalktığı için cross-coop duplicate'ler artık `Ok(None)`→replay değil **her iki coop'ta ayrı kayıt** üretir. Yorumlardaki "cross-lane collision indistinguishable" notları güncellenmeli.
- [ ] `db.rs::is_unique_violation` sınıflandırması aynen kalır (constraint adı değişebilir → mesaj/tabloname kontrolü, isim bağımlı değil).

## 3. Doğrulama (C5-3 sonrası kapıları)

- [ ] `pg_constraint`/`pg_indexes`: eski 42 global yok, 42 scoped var, 19 structural global duruyor.
- [ ] İki kooperatifte aynı `idempotency_key` + aynı request → **iki ayrı kayıt** (artık izinli; test beklentisi güncellenir — C5-2'deki "indistinguishable" testi C5-3'te tersine çevrilir).
- [ ] Aynı coop + aynı key → tek kayıt (scoped arbiter).
- [ ] Cross-coop aynı `sequence_number`/`payment_number` vb. → artık **izinli** (scoped unique) — numara testleri güncellenir; IDENTITY değerleri hâlâ global monotonic (per-coop bağımsız numaralama **M1-P3**).
- [ ] `c4_integrity` + `c5_scoped_uniques` + tam suit bounded (`--test-threads=4`).
- [ ] `kooperatif_dev`'te 0 NULL business satırı; bootstrap claim idempotent.

## 4. Rollback stratejisi

| Pencere | Strateji |
|---|---|
| 0025 transaction içinde hata | Otomatik rollback — scoped indeksler + kod C5-2 durumuna döner, sistem çalışmaya devam eder. |
| 0025 commit sonrası, aynı gün | Geri-dönüş migrasyonu: eski 42 global unique'i **yeniden oluştur** — ancak arada cross-coop duplicate üretildiyse CREATE UNIQUE INDEX **fail** olur → önce duplicate envanteri + manuel remediation gerekir (riskli pencere). |
| Veri çoğaldıktan sonra | Geri dönüş pratik değil → forward-fix. Bu yüzden §0 backup ön-kapısı zorunlu. |

## 5. Açıkça kapsam dışı (C5-3'te değil)

- Bağımsız per-cooperative doküman numaralandırma → **M1-P3** (IDENTITY sequence'leri global monotonic kalır).
- PostgreSQL RLS politikaları → M1-P3+.
- NULL-lane kayıtların fiziksel retirement'ı → M1-P3 kapısı.
- Frontend / route değişikliği → yok.
