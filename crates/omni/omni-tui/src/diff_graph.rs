//! Dokunulan dosyalarin `+`/`-` grafigi (MASTER-PLAN 9.3 / 5.2).
//!
//! "Kayip opencode ozelligi geri gelir": her ajanin dokundugu dosyalar ve
//! eklenen/silinen satir hacmi. Veri kaynagi tek: `StateEvent::FileTouched`
//! akisindan biriken [`AgentDiffStat`]. Ayni sayilar WebUI'da da vardir;
//! burada yalnizca terminal cizimi yapilir (K7).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::state::{AgentDiffStat, UiState};

/// Etiket sutununun en fazla genisligi.
const LABEL_WIDTH: u16 = 28;

/// Sayi sutunlarinin (`+nnnn -nnnn`) genisligi.
const COUNTS_WIDTH: u16 = 14;

/// Bar sutununun en az genisligi; altina duserse bar cizilmez.
const MIN_BAR_WIDTH: u16 = 4;

/// Dolu bar hucresi.
const BAR_CELL: &str = "█";

/// Calisma alani disi dokunusu isaretleyen onek.
const OUTSIDE_MARK: &str = "!";

/// Grafikte tek satir.
#[derive(Debug, Clone)]
pub struct DiffRow {
    /// Satir etiketi (ajan kimligi + persona ya da dosya yolu).
    pub label: String,
    /// Eklenen satir sayisi.
    pub added: u64,
    /// Silinen satir sayisi.
    pub removed: u64,
    /// Calisma alani disina dokunuldu mu (K5 + 5.2)?
    pub outside_workspace: bool,
}

impl DiffRow {
    /// Bar olceginde kullanilan hacim.
    pub fn volume(&self) -> u64 {
        self.added.saturating_add(self.removed)
    }
}

/// `+`/`-` bar grafigi.
#[derive(Debug, Clone)]
pub struct DiffGraph {
    title: String,
    rows: Vec<DiffRow>,
}

impl DiffGraph {
    /// Verilen satirlarla grafik kurar (hacme gore azalan siralanir).
    pub fn new(title: impl Into<String>, mut rows: Vec<DiffRow>) -> Self {
        rows.sort_by(|sol, sag| {
            sag.volume()
                .cmp(&sol.volume())
                .then_with(|| sol.label.cmp(&sag.label))
        });
        Self {
            title: title.into(),
            rows,
        }
    }

    /// Ajan basina toplam diff grafigi (9.3 ana gorunum).
    pub fn per_agent(state: &UiState) -> Self {
        let rows = state
            .diffs()
            .iter()
            .map(|(agent_id, stat)| {
                let persona = state
                    .agent(*agent_id)
                    .map(|a| a.persona.as_str())
                    .unwrap_or("?");
                DiffRow {
                    label: format!("#{agent_id} {persona}"),
                    added: stat.added,
                    removed: stat.removed,
                    outside_workspace: stat.outside_workspace > 0,
                }
            })
            .collect();
        Self::new(" Diff akisi — ajan basina ", rows)
    }

    /// Tek ajanin dosya kirilimi.
    pub fn per_file(stat: &AgentDiffStat, limit: usize) -> Self {
        let rows = stat
            .ranked()
            .into_iter()
            .take(limit)
            .map(|(yol, dosya)| DiffRow {
                label: kisalt_yol(yol, LABEL_WIDTH as usize),
                added: dosya.added,
                removed: dosya.removed,
                outside_workspace: dosya.outside_workspace,
            })
            .collect();
        Self::new(" Dokunulan dosyalar ", rows)
    }

    /// Satir sayisi.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Hic satir yok mu?
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Grafigi cizer.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.rows.is_empty() {
            let bos = Paragraph::new(Span::styled(
                "henuz dosya dokunusu yok",
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(bos, inner);
            return;
        }

        let tavan = self.rows.iter().map(DiffRow::volume).max().unwrap_or(0);
        let bar_genislik = inner
            .width
            .saturating_sub(LABEL_WIDTH + COUNTS_WIDTH + 2)
            .min(48);

        let satirlar: Vec<Line<'static>> = self
            .rows
            .iter()
            .take(inner.height as usize)
            .map(|row| self.satir_ciz(row, tavan, bar_genislik))
            .collect();

        frame.render_widget(Paragraph::new(satirlar), inner);
    }

    /// Tek satirin span'lerini uretir.
    fn satir_ciz(&self, row: &DiffRow, tavan: u64, bar_genislik: u16) -> Line<'static> {
        let etiket = kisalt_yol(&row.label, LABEL_WIDTH as usize);
        let mut spans = Vec::with_capacity(6);

        if row.outside_workspace {
            spans.push(Span::styled(
                OUTSIDE_MARK,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(" "));
        }

        spans.push(Span::raw(format!(
            "{:<width$} ",
            etiket,
            width = LABEL_WIDTH as usize
        )));
        spans.push(Span::styled(
            format!("+{:<5}", row.added),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::styled(
            format!("-{:<5} ", row.removed),
            Style::default().fg(Color::Red),
        ));

        if bar_genislik >= MIN_BAR_WIDTH {
            let (ekle, sil) = bar_hucreleri(row, tavan, bar_genislik);
            spans.push(Span::styled(
                BAR_CELL.repeat(ekle),
                Style::default().fg(Color::Green),
            ));
            spans.push(Span::styled(
                BAR_CELL.repeat(sil),
                Style::default().fg(Color::Red),
            ));
        }

        Line::from(spans)
    }
}

/// Bir satirin `+`/`-` bar hucre sayilarini hesaplar.
///
/// Tavan sifirsa ya da satir bossa bar cizilmez. Hacmi sifirdan buyuk her satir
/// en az bir hucre alir ki "dokunuldu ama az" gorunur kalsin.
fn bar_hucreleri(row: &DiffRow, tavan: u64, genislik: u16) -> (usize, usize) {
    let hacim = row.volume();
    if tavan == 0 || hacim == 0 || genislik == 0 {
        return (0, 0);
    }
    let tavan_hucre = genislik as usize;
    let olcek = (hacim as f64 / tavan as f64) * f64::from(genislik);
    let mut toplam_hucre = olcek.round() as usize;
    if toplam_hucre == 0 {
        toplam_hucre = 1;
    }
    if toplam_hucre > tavan_hucre {
        toplam_hucre = tavan_hucre;
    }

    let ham_ekle = ((row.added as f64 / hacim as f64) * toplam_hucre as f64).round() as usize;
    let ekle = if ham_ekle > toplam_hucre {
        toplam_hucre
    } else {
        ham_ekle
    };
    let sil = toplam_hucre - ekle;
    (ekle, sil)
}

/// Uzun yollari bastan kirpar; son bilesenler daha bilgilendiricidir.
fn kisalt_yol(yol: &str, genislik: usize) -> String {
    let uzunluk = yol.chars().count();
    if uzunluk <= genislik {
        return yol.to_string();
    }
    let kalan = genislik.saturating_sub(1);
    let atlanacak = uzunluk.saturating_sub(kalan);
    let kuyruk: String = yol.chars().skip(atlanacak).collect();
    format!("…{kuyruk}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use omni_proto::{FileTouch, StateEvent};

    fn satir(label: &str, added: u64, removed: u64) -> DiffRow {
        DiffRow {
            label: label.into(),
            added,
            removed,
            outside_workspace: false,
        }
    }

    #[test]
    fn satirlar_hacme_gore_siralanir() {
        let graph = DiffGraph::new(
            " t ",
            vec![satir("a", 1, 0), satir("b", 50, 10), satir("c", 5, 5)],
        );
        assert_eq!(graph.len(), 3);
        assert_eq!(graph.rows[0].label, "b");
        assert_eq!(graph.rows[1].label, "c");
    }

    #[test]
    fn bar_tavana_gore_olceklenir() {
        let en_buyuk = satir("b", 30, 10);
        let (ekle, sil) = bar_hucreleri(&en_buyuk, 40, 20);
        assert_eq!(ekle + sil, 20);
        assert_eq!(ekle, 15);
        assert_eq!(sil, 5);
    }

    #[test]
    fn kucuk_satir_en_az_bir_hucre_alir() {
        let kucuk = satir("a", 1, 0);
        let (ekle, sil) = bar_hucreleri(&kucuk, 1000, 20);
        assert_eq!(ekle + sil, 1);
        assert_eq!(ekle, 1);
    }

    #[test]
    fn bos_satir_bar_cizmez() {
        let bos = satir("a", 0, 0);
        assert_eq!(bar_hucreleri(&bos, 10, 20), (0, 0));
        assert_eq!(bar_hucreleri(&satir("a", 5, 5), 0, 20), (0, 0));
    }

    #[test]
    fn uzun_yol_bastan_kirpilir() {
        let kisa = kisalt_yol("a/b.rs", 20);
        assert_eq!(kisa, "a/b.rs");
        let uzun = kisalt_yol("crates/omni/omni-tui/src/diff_graph.rs", 20);
        assert_eq!(uzun.chars().count(), 20);
        assert!(uzun.starts_with('…'));
        assert!(uzun.ends_with("diff_graph.rs"));
    }

    #[test]
    fn ajan_grafigi_akistan_beslenir() {
        let mut state = UiState::new();
        for (agent_id, path, added, removed, disari) in [
            (1_i64, "src/a.rs", 10_u32, 2_u32, false),
            (1, "src/b.rs", 4, 0, false),
            (2, "/etc/hosts", 1, 1, true),
        ] {
            state.apply_event(StateEvent::FileTouched(FileTouch {
                id: None,
                agent_id,
                path: path.into(),
                outside_workspace: disari,
                added,
                removed,
                pre_ref: None,
                post_ref: None,
                ts: omni_proto::now(),
            }));
        }

        let graph = DiffGraph::per_agent(&state);
        assert_eq!(graph.len(), 2);
        assert_eq!(graph.rows[0].added, 14);
        assert_eq!(graph.rows[0].removed, 2);
        assert!(graph.rows[1].outside_workspace);

        let stat = state.diff(1).expect("diff");
        let dosyalar = DiffGraph::per_file(stat, 10);
        assert_eq!(dosyalar.len(), 2);
        assert!(dosyalar.rows[0].label.ends_with("a.rs"));
    }
}
