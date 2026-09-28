/**
 * Turkish (tr-TR) dictionary — the default and fallback product language.
 * Keys are the single source of truth for translatable messages.
 */
export const trTR = {
	'a11y.skipToContent': 'İçeriğe geç',
	'app.title': 'Kooperatif',
	'app.subtitle': 'Kooparatif Yönetim Sistemi — temel altyapı kabuğu',
	'nav.label': 'Ana gezinti',
	'nav.placeholder': 'Gezinti menüsü sonraki uygulama adımlarında eklenecek.',
	'home.welcome.title': 'Hoş geldiniz',
	'home.welcome.body':
		'Bu ekran, STEP-001 kapsamında oluşturulan temel uygulama kabuğudur. Etki alanı ekranları (Hissedarlar, Dönemler, Tahsilat, Finans vb.) sonraki onaylanmış adımlarda uygulanacaktır.',
	'health.title': 'Sistem Durumu',
	'health.description': 'Arka uç servis ve altyapı bileşenlerinin durumu.',
	'health.refresh': 'Yeniden Denetle',
	'health.loading': 'Denetleniyor…',
	'health.api.label': 'API servisi',
	'health.api.ok': 'Erişilebilir',
	'health.api.unreachable': 'Erişilemiyor',
	'health.db.label': 'PostgreSQL',
	'health.db.ok': 'Hazır',
	'health.db.unconfigured': 'Yapılandırılmadı',
	'health.db.unavailable': 'Erişilemiyor',
	'auth.login.title': 'Oturum Aç',
	'auth.login.description': 'Devam etmek için kullanıcı bilgilerinizi girin.',
	'auth.login.username': 'Kullanıcı adı',
	'auth.login.password': 'Parola',
	'auth.login.submit': 'Giriş Yap',
	'auth.login.submitting': 'Giriş yapılıyor…',
	'auth.login.error.authentication_failed': 'Kullanıcı adı veya parola hatalı.',
	'auth.login.error.rate_limited':
		'Çok fazla başarısız deneme. Lütfen bir süre sonra tekrar deneyin.',
	'auth.login.error.csrf_failed':
		'Güvenlik doğrulaması başarısız. Sayfayı yenileyip tekrar deneyin.',
	'auth.login.error.dependency_unavailable':
		'Sistem geçici olarak kullanılamıyor. Lütfen daha sonra tekrar deneyin.',
	'auth.login.error.validation_failed': 'Lütfen kullanıcı adı ve parola alanlarını doldurun.',
	'auth.login.error.fallback': 'Oturum açılamadı. Lütfen tekrar deneyin.',
	'auth.shell.logout': 'Çıkış Yap',
	'auth.shell.sessions': 'Oturumlarım',
	'auth.sessions.title': 'Oturumlarım',
	'auth.sessions.description':
		'Hesabınızdaki aktif oturumlar aşağıda listelenir. Tanımadığınız bir oturum görürseniz kapatın.',
	'auth.sessions.current': 'Bu oturum',
	'auth.sessions.client': 'İstemci',
	'auth.sessions.status': 'Durum / İşlem',
	'auth.sessions.createdAt': 'Oturum açılma',
	'auth.sessions.lastSeenAt': 'Son etkinlik',
	'auth.sessions.expiresAt': 'Geçerlilik bitişi',
	'auth.sessions.revoke': 'Oturumu Kapat',
	'auth.sessions.revokeOthers': 'Diğer Tüm Oturumları Kapat',
	'auth.sessions.empty': 'Başka aktif oturum bulunmuyor.',
	'auth.sessions.loading': 'Yükleniyor…',
	'auth.sessions.revoking': 'Kapatılıyor…'
} as const;

export type MessageKey = keyof typeof trTR;
