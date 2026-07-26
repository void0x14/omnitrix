-- Faz 3 kolonları · bütçe zarfı · süre hedefi · ajan katmanı (MASTER-PLAN 14.2)
-- 0003 tabloları KORUNUR; sadece eksik kolonlar eklenir.
-- SQLite ALTER TABLE ADD COLUMN kısıtı: varsayılansız NOT NULL eklenemez,
-- bu yüzden tüm kolonlar nullable. CHECK'li kolon eklenebilir (NULL geçer).

-- AS4 · bütçe zarfı: ebeveynin tahsisi ve çocuğun harcaması (bkz. 6.4)
ALTER TABLE tasks ADD COLUMN budget_allocated REAL;
ALTER TABLE tasks ADD COLUMN budget_spent REAL;

-- AS13 · problem-süre sistemi: kural tabanlı skorer çıktısı (mvp|full)
ALTER TABLE tasks ADD COLUMN duration_target TEXT
    CHECK (duration_target IS NULL OR duration_target IN ('mvp', 'full'));

-- 3.1 · ajan katmanı: existing/sleeping/queued/active
ALTER TABLE agents ADD COLUMN tier TEXT
    CHECK (tier IS NULL OR tier IN ('existing', 'sleeping', 'queued', 'active'));

-- Uyku bağlamı CAS atfı (omni-storage/cas.rs içerik-hash'i)
ALTER TABLE agents ADD COLUMN context_ref TEXT;

-- 7.3 · hiyerarşi derinliği ve raporlama için bellek ayak izi
ALTER TABLE agents ADD COLUMN depth INTEGER;
ALTER TABLE agents ADD COLUMN rss_kb INTEGER;

-- Katman/derinlik sorguları sık: zamanlayıcı tier taraması için indeks
CREATE INDEX IF NOT EXISTS idx_agents_tier ON agents(tier);
CREATE INDEX IF NOT EXISTS idx_tasks_duration_target ON tasks(duration_target);
