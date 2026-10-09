# UI-TR-001 — Turkish UX Copy & Terminology Audit

Scope: presentation-layer copy only. No schema, contract, calculation,
authorization, workflow, or idempotency changes.

## A. Screen Inventory

Audited via route tree `apps/web/src/routes` and shared dictionary
`apps/web/src/lib/i18n/tr-TR.ts` (~1,400 keys):

| Area                     | Routes                                | Notes                                                                                            |
| ------------------------ | ------------------------------------- | ------------------------------------------------------------------------------------------------ |
| Login / auth             | `login`                               | Already Turkish; unchanged                                                                       |
| Dashboard                | `(app)/+page.svelte`                  | Placeholder copy rewritten (STEP-001 scaffolding text removed)                                   |
| Persons / Shareholders   | `hissedarlar`                         | Inherits payment/credit/return sections                                                          |
| Families                 | `aileler`                             | Empty state improved                                                                             |
| Shares                   | `hisseler`                            | Description now explains the share concept                                                       |
| Periods / Assessments    | `donemler`, `tahakkuklar`             | Form labels switched to "Aidat" wording                                                          |
| Payments / Allocations   | `tahsilatlar`                         | Allocated/unallocated/reversal wording clarified                                                 |
| Credits (excess/advance) | shareholder + assessment detail       | "Mahsup" → "Borçtan Düşme" vocabulary                                                            |
| Financial accounts       | `finansal-hesaplar`                   | Account/movement/restriction wording                                                             |
| Transfers                | `transferler`                         | "Ters Kaydet" → "Geri Al"                                                                        |
| Income / Expense         | `gelirler`, `giderler`, `gelir-gider` | Same reversal wording; conflict messages                                                         |
| Share returns            | `hisse-iadeleri`                      | Entitlement + settlement wording                                                                 |
| Investments              | `yatirimlar`                          | Funding/valuation/income/disposal labels kept distinct from operational income/expense           |
| Social Aid               | `sosyal-yardim`                       | "Tahsisli" → "yardım için ayrılan" wording throughout                                            |
| Governance               | `yonetim`                             | Save buttons made context-specific (draft vs body vs membership)                                 |
| Reporting hub            | `raporlar`                            | Metric labels aligned with TV                                                                    |
| Live TV                  | `(tv)/canli-ekran`                    | Regrouped into 3 sections + per-metric help text                                                 |
| Users / Roles            | `kullanicilar`, `roller`              | Description rewrite; `roles.categoryOther` key added; role-name conflict gets a specific message |

## B. Terminology Dictionary

| Eski etiket                   | Yeni etiket                                                        | Gerekçe                                   | Altında yatan anlam                                                    |
| ----------------------------- | ------------------------------------------------------------------ | ----------------------------------------- | ---------------------------------------------------------------------- |
| Tahakkuk Toplamı              | Toplam Aidat Borcu                                                 | "Tahakkuk" teknik; üye dili "aidat borcu" | Dönemlerde hisselere çıkarılan borç toplamı (tahsil edilmiş değil)     |
| Kaydedilmiş Ödemeler          | Yapılan Ödemeler                                                   | Sadeleştirme                              | Sisteme kaydedilen tüm ödemeler; bir kısmı borca dağıtılmamış olabilir |
| Açık Tahakkuk Borcu           | Ödenmemiş Aidatlar                                                 | Sadeleştirme                              | Henüz kapatılmamış aidat borcu bakiyesi                                |
| Finansal Hesap Bakiyeleri     | Kasalardaki Toplam Para                                            | Sadeleştirme                              | Tüm finansal hesapların hareket toplamlarından türeyen bakiye          |
| Faaliyet Geliri               | Diğer Gelirler                                                     | Aidat tahsilatı karışmasın                | Aidat dışı operasyonel gelir                                           |
| Faaliyet Gideri               | Yapılan Giderler                                                   | Sadeleştirme                              | Operasyonel giderler (iade, yatırım finansmanı, yardım hariç)          |
| Bağış Toplamı                 | Toplanan Bağışlar                                                  | Sadeleştirme                              | Sosyal yardım fonlarına kaydedilen bağışlar                            |
| Yardım Ödemesi Toplamı        | Yapılan Yardımlar                                                  | Sadeleştirme                              | Fondan yapılan yardım ödemeleri                                        |
| Sosyal Yardım Tahsisli Bakiye | Yardım İçin Ayrılan Para                                           | "Tahsisli" teknik                         | Kasa parasının sosyal yardıma ayrılmış bölümü — ek nakit değil         |
| Mahsup / Avansla kapat        | Borçtan Düşme                                                      | Sadeleştirme                              | Mevcut kredinin borca uygulanması; nakit hareketi yok                  |
| Ters Kayıt / Ters Kaydet      | Geri Alma / Geri Al                                                | Günlük dil                                | Reversal kaydı; tarih korunur, silme yok                               |
| Kaydet (bağlamsız)            | Ödemeyi Kaydet, Taslağı Kaydet, Kurulu Oluştur, Üyeliği Kaydet vb. | Belirsiz buton metni                      | İşleme özel fiil                                                       |

Blind-replacement korunması: "Tahakkuk" hâlâ `tahakkuklar` route'unda ve
yasal/kayıt bağlamında durabilir; "Avans" `credits.*` başlığında korundu
("Fazla Ödeme / Avans"); "Dağılım" ödeme→borç bağlama eylemi olarak korundu.

## C. TV Dashboard (Priority Screen)

Dokuz metrik, üç aria-labelledby bölümüne ayrıldı:

- **AİDAT DURUMU**: Toplam Aidat Borcu / Yapılan Ödemeler / Ödenmemiş Aidatlar
- **KASA VE FAALİYETLER**: Kasalardaki Toplam Para / Diğer Gelirler / Yapılan Giderler
- **SOSYAL YARDIMLAR**: Toplanan Bağışlar / Yapılan Yardımlar / Yardım İçin Ayrılan Para

Her kartta görünür kısa açıklama (`tv.help.*`) — tooltip değil, görünür metin
(dokunmatik/uzak ekran uyumu). Değerler ve `data-testid` hook'ları birebir
korundu; mode filtreleri (`collection`/`accounts`/`aid`) davranışı değişmedi.
Bağlantı durumu, son eşitleme, stale rozeti korundu.

## D. Errors / Empty States

- `errors.conflict`: jenerik ama eylem odaklı metin (sayfayı yenile + kontrol et).
- `errors.insufficient_unrestricted_funds`: "Bu kasada işlem için yeterli
  kullanılabilir para yok. Paranın bir bölümü sosyal yardımlar için ayrılmış olabilir."
- `errors.lockout_prevented`: son yönetim kullanıcısı koruması açık dille anlatılıyor.
- `roles.errors.nameConflict` (yeni): rol adı çakışmasına özel mesaj.
- Tüm liste empty-state'leri: ne gösterildiği + ilk adım önerisi
  ("Aramayı değiştirebilir veya 'Yeni X' ile kayıt oluşturabilirsiniz.").
- Aksiyon görünürlüğü permission'a bağlı kaldı (spec'ler doğruluyor).

## E. Accessibility

- TV grup başlıkları `aria-labelledby` ile section'a bağlı.
- Açıklamalar tooltip yerine görünür kısa metin (dokunmatik + uzak okuma).
- Renk tek statü sinyali değil: metin rozetleri + border + açıklama korundu.
- `t('roles.categoryOther')` ile son hard-coded Türkçe literal kaldırıldı.

## F. Tests

- `i18n.spec.ts`: +6 UI-TR-001 blok testi (TV etiketleri, grup başlıkları,
  help metinlerinin varlığı + min uzunluk, aidat/ödeme/geri-alma ayrımı,
  restricted-fund açıklaması, en override + tr fallback).
- 9 mevcut spec yeni etiketlere güncellendi (label/placeholder/text assertion'ları).
- Sonuç: **Vitest 168/168 PASS**, svelte-check 0/0, ESLint+Prettier clean,
  production build PASS.

## G. Member-Friendly Glossary (yardım sayfası taslağı)

| Terim                    | Anlamı                                                                          |
| ------------------------ | ------------------------------------------------------------------------------- |
| Hisse                    | Kooperatifteki katılım birimi; aidat borcu ve sahiplik bunun üzerinden izlenir. |
| Hissedar                 | Hissenin o anki sahibi; sahiplik zamanla el değiştirebilir.                     |
| Dönem                    | Aidat borcunun oluşturulduğu dönem.                                             |
| Aidat Borcu              | Hissedarın ödemesi gereken tutar; henüz tahsil edilmiş değildir.                |
| Tahsilat                 | Sisteme kaydedilen ödeme.                                                       |
| Fazla Ödeme / Avans      | Ödenenin borca dağıtılmayan kısmı; ilerideki borçlara kullanılabilir.           |
| Borçtan Düşme            | Mevcut avansın borca uygulanması; nakit hareketi yoktur.                        |
| Kasa / Finansal Hesap    | Sistemde izlenen nakit veya banka hesabı.                                       |
| Hisse İadesi             | Ayrılan hissedarın anapara + varsa kâr payı hakkı.                              |
| Yatırım                  | Fona ayrılan para; değerleme bilgi amaçlıdır, nakit değildir.                   |
| Sosyal Yardım Fonu       | Bağışların ayrıldığı, yalnızca yardım amacıyla harcanabilen para havuzu.        |
| Yardım İçin Ayrılan Para | Kasadaki paranın yardımlar için kilitlenen bölümü.                              |
| Geri Alma                | Yanlış kaydın ters kayıtla düzeltilmesi; kayıt silinmez, izi kalır.             |
| Yönetim Kararı           | Kurul tarafından kaydedilen karar; sistem hukuki yeterliliği doğrulamaz.        |

## H. Ambiguities / Owner Decisions

1. `reports.ov.postedPayments` → "Yapılan Ödemeler": ödemenin tümü aidata
   gitmeyebilir (fazla ödeme/avans kalabilir). Açıklama metni bunu not eder;
   "Tahsil Edilen Aidat" etiketi yanlış olurdu — seçilmedi.
2. `operationalIncome` → "Diğer Gelirler": yatırım geliri ve bağışlar bu
   toplama dahil değildir; açıklama metni "aidat tahsilatı dışında" diye
   sınırlar. Daha kesin isim isterse "Faaliyet Gelirleri (aidat dışı)" adayı
   sahibi onayına açık.
3. "Dönem" vs "Aidat Dönemi": anahtar `periods.title` "Dönemler" kaldı;
   form alanlarında "Aidat" kullanıldı. Tam birleşme için sahibi kararı.
4. `credits.title` "Fazla Ödeme / Avans" ikilisi korundu — tek terim seçimi
   sahibi tercihine bırakıldı.

## I. Remaining Issues

- Production DR, F8 collection-session scope, RPO/RTO — UI-TR-001 dışı,
  PILOT-FIX-001 closure'da takipte.
- Uzun Türkçe etiketler mobilde test edildi mi: metinler `<p>` içinde
  doğal wrap; ayrıca viewport E2E'si RC E2E kapsamında (değişiklik öncesi
  PASS). TV grid `grid-cols-3` sabit — TV senaryosu için bilinçli; dar
  ekranda yatay kayma riski not edilir (canlı-ekran hedefi projektör).
