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
	'health.db.unavailable': 'Erişilemiyor'
} as const;

export type MessageKey = keyof typeof trTR;
