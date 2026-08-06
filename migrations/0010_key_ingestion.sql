-- FAZ 8 · anahtar besleme hattı (K13, MASTER-PLAN 6.3)
-- Harici anahtar DB'sinden beslenen kayıtlar. GERCEK anahtar ASLA DB'de
-- tutulmaz; yalnizca SHA-256 parmak izi (`key_ref`). Canlı/ölü ayrımı
-- `fed_keys.status` kolonundadır; ölüler ayrı `dead_keys` bölümüne
-- yansıtılır (ileride canlanabilir — revizable, 12.2).

-- Canlı/ölü anahtar kayıtları (besleme hattının tek gerçek kaynağı).
CREATE TABLE IF NOT EXISTS fed_keys (
    key_ref       TEXT PRIMARY KEY,
    provider_kind TEXT,
    base_url      TEXT,
    status        TEXT NOT NULL CHECK(status IN ('live','dead')),
    source_path   TEXT NOT NULL,
    label         TEXT,
    verify_tier   TEXT NOT NULL,
    detail        TEXT,
    first_seen    TEXT NOT NULL,
    last_checked  TEXT NOT NULL
);

-- Ölü bölümü: `fed_keys`'teki ölü satırların kalıcı yansıması. Bir anahtar
-- canlıya dönerse `fed_keys.status` güncellenir; `dead_keys` tarihçe olarak
-- kalır.
CREATE TABLE IF NOT EXISTS dead_keys (
    key_ref       TEXT PRIMARY KEY,
    provider_kind TEXT,
    base_url      TEXT,
    source_path   TEXT NOT NULL,
    label         TEXT,
    verify_tier   TEXT NOT NULL,
    detail        TEXT,
    first_seen    TEXT NOT NULL,
    last_checked  TEXT NOT NULL
);

-- /omni-keys özeti ve router dağılımı sık sorgulanır.
CREATE INDEX IF NOT EXISTS idx_fed_keys_status ON fed_keys(status);
CREATE INDEX IF NOT EXISTS idx_fed_keys_provider ON fed_keys(provider_kind);
