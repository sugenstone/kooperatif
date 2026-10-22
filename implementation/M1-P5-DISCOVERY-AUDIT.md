# M1-P5 — Realtime ve Audit Tenant İzolasyonu — Keşif ve Güvenlik Denetimi

**Adım:** P5-01 (salt-okunur keşif) — **Tarih:** 2026-10-18 — **Taban:** migration 0060 (v60), M1-P4 + BR1 tamamlanmış, M1 kimlik/üyelik ayrımı uygulanmış.

Bu belge kod değiştirmez; doğrulanmış mimari envanteri, kanıtlanmış bulguları ve önerilen iyileştirme batch'lerini içerir. Tüm sorgular disposable DB'lerde (`kooperatif_backup_drill_restore`, v60) koşturuldu.

---

## 1. Baseline doğrulaması

| Kontrol | Kanıt |
|---|---|
| Son migration | `0060_m1_identity_membership_separation.sql`; `_sqlx_migrations` = 60 (drill restore DB'de doğrulandı) |
| RLS sınırı | 43 tabloda FORCE RLS; `security_events` dahil (0049). `kooperatif_app` SELECT/INSERT'ı `security_events`'ta **yok** (canlı: `f/f`) |
| Runtime rolü | `kooperatif_app` — NOSUPERUSER/NOBYPASSRLS; `pg_notify` EXECUTE hakkı var (PUBLIC default — **bulgu RT-2**) |
| Oturum/üyelik | `app_session_resolve` → `app_session_membership` → `verify_business_membership`; suspend/ended her katmanda reddedilir |
| Realtime | LISTEN/NOTIFY + in-process broadcast + tek WS endpoint (`/api/realtime`) |
| Audit | 35 audit trigger'ı + sealed definer yazım şeritleri; **okuma API'si yok** |

## 2. Realtime kanal envanteri

**Tek mekanizma** — SSE, long-poll, background-job yayını **yok**:

```
DB commit → rt_* AFTER trigger (38 tablo) → pg_notify('kooperatif_domain_changed', <domain_tag>)
  → listen_loop (özel PG bağlantısı, kooperatif_app; reconnect + Resync)
  → RealtimeHub (tokio broadcast, kapasite 256, Lagged→Resync)
  → WS client → domain invalidation → canonical REST refetch
```

| Özellik | Durum |
|---|---|
| Handshake | `BusinessAuth` (tam session + coop üyelik doğrulaması) + koşulsuz Origin allowlist + cookie-only token |
| Payload | Yalnız domain tag — satır/kişi/tutar **asla** yok (minimization, doğrulandı: `prosrc` = `pg_notify(channel, TG_ARGV[0])`) |
| Subscribe | `{type:'subscribe', scopes:[perm]}` — server `known_scopes ∩ effective_perms` ile daraltır; cevap `Subscribed{granted}` |
| Teslimat filtresi | **Her sinyalde** `revalidate()`: `classify_session` + `verify_business_membership` + taze scoped perms — oturum/üyelik/yetki kaybı → close 1008 |
| Heartbeat | 30 sn — sinyal olmasa bile oturum iptali/disable socket'i öldürür |
| Reconnect | Client: bounded exponential backoff (10 deneme), visibility/online wake, stall-force-cycle 90sn; her reconnect'te `'*'` resync |
| Switching | Socket handshake scope'una sabit (header = bootstrap coop; tarayıcı WS header gönderemez, hep headerless→bootstrap pin) |

Frontend tüketiciler: `(app)/+layout.svelte` (auth.permissions'a göre tüm scope'lar), `(tv)/canli-ekran` (`reports.read` — aynı cookie auth, gizli modda değil), `report-panel.svelte`.

## 3. Audit yüzeyi envanteri

**Yazım şeritleri (hepsi definer, `kooperatif_app` EXECUTE'lu, PUBLIC revoke'lu):**

| Şerit | Fonksiyon | Atomiklik | Atıf |
|---|---|---|---|
| Mühürlü tx | `app_ctx_audit` | İşlemle atomik | `app.*` GUC seal'inden türetilir (ctx_seal/user_seal doğrulaması) |
| Session-platform | `app_session_audit_platform` | Best-effort ayrı tx | token_hash'ten user/session; coop=NULL zorunlu |
| Session-scoped | `app_session_audit_scoped` | Best-effort | **yalnız `lockout_prevented`**; coop üyeliği DB'de yeniden doğrulanır |
| Login-fail | `app_audit_login_failed` | Best-effort | Anonim; event_type + atıf sabit |
| Owner (CLI) | `record`/`record_scoped` | Owner tx | Açık parametre — runtime'dan erişilemez |
| **Trigger** | `app_audit_transition` + 35 `*_audit` trigger + `cooperative_memberships_audit` (0060) | Satır değişikliğiyle atomik | sealed ctx / atıf |

**Audit okuma:** `security_events` için `kooperatif_app`'in SELECT/INSERT/UPDATE/DELETE'i **yok**, route'da okuma endpoint'i **yok**, frontend ekranı **yok**. Tarihçe yalnız owner bağlantısıyla okunabilir. Retention: silme/purge mekanizması yok (append-only tasarım).

## 4. Kanıtlanmış bulgular

### RT-1 — Cross-cooperative sinyal sızıntısı (ORTA, tasarım)

`pg_notify` payload'ında `cooperative_id` **yok**; `RealtimeHub` tek global yayın kanalı. Teslimat filtresi yalnız *okuma izni* — scope süzgeci yok. Bugün tek-coop'ta zararsız ama mimari çok-kooperatifli: **ikinci kooperatif aktive edildiğinde**, A'daki bir `payments` commit'i B scope'lu socket'e `data-changed:payments` teslim eder. Veri sızıntısı yok (payload tag-only, client refetch'i scoped API'den) ama **başka kooperatifin yazma aktivitesini/zamanlamasını gözlemleme** mümkün (signal oracle) + tüm tenant'ların client'larında gereksiz refetch fırtınası.

### RT-2 — Sahte NOTIFY enjeksiyonu (DÜŞÜK-ORTA)

`kooperatif_app` `pg_notify('kooperatif_domain_changed', 'payments')` çağırabilir — **canlı kanıtlandı**. Etki: sahte invalidation → tüm socket'lerde revalidate DB round-trip + client refetch; yüksek frekansta **amplifikasyon DoS** veya stale-UI gürültüsü. Satır verisi sızdırmaz.

### RT-3 — Suspend/kapama gecikmesi penceresi (DÜŞÜK, kabul edilebilir)

Socket ölümü sinyal-başına revalidate **veya** 30 sn heartbeat'e bağlı — suspend edilen üyenin socket'i en kötü ~30 sn + sonraki sinyal anında yaşar (daha önce `membership_suspension_stops_delivery_and_reconnect` testiyle kanıtlı). Sessiz dönemde ≤30 sn stale-socket penceresi mevcut.

### RT-4 — Üyelik/rol değişiklikleri realtime sinyal üretmez (DÜŞÜK)

38 `rt_*` trigger'ı domain tablolarında; `cooperative_memberships`, `users`, `roles`, `role_permissions` üzerinde yok. Üyelik suspend edilen kullanıcının *diğer* client'larına "yetki değişti" invalidation'ı gitmez — UI ancak yeniden-login/sayfa-yüklemede öğrenir (backend zaten her istekte reddediyor).

### AU-1 — Audit okuma API'si yok (ürün boşluğu — güvenlik açığı değil)

`security_events` uygulama katmanından tamamen kapalı (owner-lane only). Cross-coop audit görünürlüğü riski sıfır, ama kooperatif admini kendi coop'unun audit'ini **göremez** — docs/19 ve 18 audit-viewer gereksinimi varsa eksik.

## 5. Doğrulanmış güvenlik pozitifleri

- **Handshake**: `BusinessAuth` — suspended/ended üye socket açamaz; açık `x-cooperative-id` yalnız bootstrap coop'a eşitse kabul (sahte coop id → 403); Origin allowlist koşulsuz (WS CSRF muafiyeti kapatılmış).
- **Session revocation/global disable**: per-signal revalidate + 30sn heartbeat → close 1008 (testlerde kanıtlı).
- **Forged scope subscribe**: `known_scopes ∩ live perms` — bilinmeyen/yetkisiz scope sessizce düşürülür.
- **Payload minimization**: `DataChanged{domain,occurredAt}` — testler envelope şeklini sabitliyor.
- **Audit bütünlüğü**: tüm yazım şeritleri sealed/verified; `app_session_audit_scoped` tek event-type'a kilitli; trigger mint'i tx-atomik; `kooperatif_app` audit'i okuyamaz/yazamaz (definer dışında).
- **RLS**: realtime hiç satır taşımaz — refetch hep canonical scoped API'den, RLS + membership her okumada yeniden denetlenir.

## 6. Önerilen iyileştirme batch'leri

| # | Batch | Bağımlılık | Kabul kriteri |
|---|---|---|---|
| B1 | **Coop-scoped NOTIFY**: `kooperatif_notify_domain` → payload `domain:coop_id` (NEW/OLD.cooperative_id); hub `Changed(Domain, Uuid)`; teslimat `scope==Some(coop)` filtresi | Migration 0061 (append-only) | B commit'i A socket'ine ulaşmaz; restricted test kanıtı |
| B2 | **NOTIFY forgery azaltımı**: payload'a coop-id eklendikten sonra trigger-emit eden definer/constraint veya imzalı tag | B1 | `kooperatif_app` pg_notify çağrısı socket'e ulaşmaz (coop-id eşleşmez / HMAC) |
| B3 | **Üyelik/kimlik realtime domain'i**: `memberships` domain tag'ı + trigger; suspend'de ilgili clientlara invalidation | B1 | Suspend → kullanıcının açık UI'ı anında stale göstermez |
| B4 | **Audit okuma API'si** (ürün): sealed definer + `security_events` coop-scoped SELECT politikası + endpoint + UI | B1, spec onayı gerekli | Sadece kendi coop'unun eventleri; platform event'leri yalnız platform admin |
| B5 | **Suspension close latency**: suspend'i aktif socket'lere anında itmek için `user_disabled`-tarzı immediate close kanalı | B1/B3 | <1 sn'de socket kapanır, heartbeat'e kalmaz |

## 7. Gerekli restricted-role testleri (önerilen, implemente edilmedi)

- A+B üyeli kullanıcı: B scope socket'i A commit'ini **almaz** (B1 sonrası)
- `SET ROLE kooperatif_app; pg_notify(...)` → socket **teslim etmez** (B2)
- Suspend edilen üye: socket ≤1sn'de kapanır (B5)
- Audit endpoint: koop A admini B'nin event'ini okuyamaz (B4)
- Çok-coop switch: socket scope değişimi davranışı (ürün kararı: reconnect mi, per-coop socket mi?)

## 8. GO / NO-GO

**GO — koşullu.** Tek kooperatifte mevcut mimari güvenli (payload minimization + per-signal revalidate + sealed audit). İkinci kooperatif aktive edilmeden önce **B1 zorunlu**; B2/B3 güvenlik-hardening olarak önerilir. B4 ayrı ürün kararı. Mevcut realtime + audit yazım tarafında kritik veri-sızıntısı bulgusu **yok**.
