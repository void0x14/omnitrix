//! Faz 5 — shell allowlist + acil kacis (MASTER-PLAN 9.1, K3, I4).
//!
//! Model katmaninda token engelleme IMKANSIZDIR (3.5); zorlama TOOL
//! katmanindadir. Bu modul bir shell komut satirini GERCEKTEN ayristirir ve
//! calistirilabilir HER ikiliyi tek tek politikaya sorar. Tek kelime eslesmesi
//! yetmez: boru hatti, `;`/`&&`/`||` zincirleri, `$(...)` ve backtick komut
//! ikamesi, surec ikamesi `<(...)`, heredoc govdesi ve yonlendirmeler
//! ayristirilir; her birinden cikan komut ayni suzgecten gecer.
//!
//! I4: burada islem izolasyonu YOKTUR ve iddia edilmez — bu bir POLICY
//! katmanidir. Yetki tek noktada toplanir (tool broker + bu modul); karar
//! `capability_audit`'e, denenen cagri `tool_calls`'a, acil kacis talebi
//! `interrupts`'a duser.
//!
//! Akis:
//! 1. [`ShellGuard::authorize`] — komut satiri ayristirilir, [`ShellPolicy`]
//!    karar verir, karar denetime yazilir, red `ToolsError::Denied` doner.
//! 2. Native tool yetmezse ajan GEREKCE yazip [`EscapeRequest`] acar; talep
//!    `interrupts` + `capability_audit` satiri uretir (henuz izin YOK).
//! 3. YARGIC [`ShellGuard::approve_escape`] ile SURELI izin verir; sure dolunca
//!    izin kendiliginden duser. Her adim loglanir.

use std::collections::HashMap;

/// Zaman tipleri tek noktadan yeniden yayinlanir: acil kacis suresi bu tiplerle
/// konusulur ve tuketiciler (omni-storage, kapi testleri) ayri bir chrono
/// surumune bagimli kalmaz.
pub use chrono::{DateTime, Duration, Utc};

use crate::broker::{AuditGate, CapabilityDecision, Decision, ToolCallRecord};
use crate::error::ToolsError;

/// `capability_audit.capability` — shell komut calistirma yetkisi.
pub const CAPABILITY_SHELL: &str = "shell";
/// `capability_audit.capability` — acil kacis (gecici serbest shell) yetkisi.
pub const CAPABILITY_SHELL_ESCAPE: &str = "shell_escape";
/// `tool_calls.tool` — reddedilen shell cagrilarinin tool adi.
pub const SHELL_TOOL: &str = "shell";
/// `interrupts.kind` — acil kacis talebi.
pub const INTERRUPT_KIND_SHELL_ESCAPE: &str = "shell_escape";
/// `interrupts.source` — talebi ajan acti.
pub const INTERRUPT_SOURCE_AGENT: &str = "agent";
/// `interrupts.source` — kaydi yargic kapatti.
pub const INTERRUPT_SOURCE_JUDGE: &str = "judge";

/// Ic ice ikame/heredoc coz umlemesinde izin verilen en buyuk derinlik.
const MAX_DEPTH: usize = 8;
/// Tek komut satirindan cikabilecek en fazla komut sayisi.
const MAX_COMMANDS: usize = 256;
/// Ayristirilan komut satirinin en buyuk uzunlugu (karakter).
const MAX_INPUT_CHARS: usize = 64 * 1024;
/// Denetim kaydina yazilan komut metninin kirpma siniri.
const AUDIT_TARGET_LIMIT: usize = 512;
/// Acil kacis izninin ust siniri (saniye).
const DEFAULT_MAX_ESCAPE_SECS: i64 = 900;
/// Acil kacis gerekcesinin en kisa uzunlugu.
const MIN_JUSTIFICATION_CHARS: usize = 16;

// ---------------------------------------------------------------------------
// Ayristirma — sozcuk cozumleyici
// ---------------------------------------------------------------------------

/// Ayristirma hatasi. Ayristiramadigimiz metin CALISTIRILMAZ (fail-closed).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellParseError {
    /// Kapanmamis tek/cift tirnak.
    #[error("kapanmamis tirnak")]
    UnterminatedQuote,
    /// Kapanmamis `$(...)`, `${...}` veya backtick.
    #[error("kapanmamis ikame")]
    UnterminatedSubstitution,
    /// Hedefi olmayan yonlendirme.
    #[error("hedefsiz yonlendirme")]
    DanglingRedirect,
    /// Ic ice ikame derinligi asildi.
    #[error("ikame derinligi asildi")]
    TooDeep,
    /// Komut satiri kabul edilen siniri asti.
    #[error("komut satiri cok buyuk")]
    TooLarge,
}

/// Bir komutun nereden geldigi — denetim kaydinda kacis vektoru gorunur olur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandOrigin {
    /// Dogrudan komut satirinda (boru hatti / zincir dahil).
    #[default]
    Direct,
    /// `$(...)` veya backtick komut ikamesi icinde.
    CommandSubstitution,
    /// `<(...)` / `>(...)` surec ikamesi icinde.
    ProcessSubstitution,
    /// Tirnaksiz heredoc govdesinde genisleyen ikame icinde.
    HeredocExpansion,
}

impl CommandOrigin {
    /// Denetim kaydina yazilan sabit etiket.
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandOrigin::Direct => "direct",
            CommandOrigin::CommandSubstitution => "command_substitution",
            CommandOrigin::ProcessSubstitution => "process_substitution",
            CommandOrigin::HeredocExpansion => "heredoc_expansion",
        }
    }
}

/// Ayristirilmis tek bir basit komut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedCommand {
    /// Calistirilacak ikilinin duz metni (tirnak/kacis cozulmus).
    pub binary: String,
    /// Ikiliden sonraki argumanlar.
    pub argv: Vec<String>,
    /// Komut onundeki `VAR=deger` atamalari.
    pub assignments: Vec<String>,
    /// Komutun kaynagi.
    pub origin: CommandOrigin,
    /// Ikili adi genisleme icerdi mi (`$CMD`, `$(which x)`)?
    pub dynamic_binary: bool,
    /// Ikili adi yol ayraci icerdi mi (`/bin/sh`, `./x.sh`)?
    pub path_binary: bool,
    /// Ic ice ikame derinligi.
    pub depth: usize,
}

/// Ayristirilmis yonlendirme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirection {
    /// Operator metni (`>`, `>>`, `2>`, `<`, `<<<`, `&>` ...).
    pub op: String,
    /// Yonlendirme hedefi.
    pub target: String,
}

/// Ayristirilmis heredoc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heredoc {
    /// Sonlandirici etiket.
    pub tag: String,
    /// Etiket tirnakli miydi (tirnakli = govdede genisleme YOK)?
    pub quoted: bool,
    /// `<<-` bicimi mi (bastaki sekmeler kirpilir)?
    pub strip_tabs: bool,
    /// Heredoc govdesi.
    pub body: String,
}

/// Bir komut satirinin tam ayristirma sonucu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellPlan {
    /// Bulunan butun calistirilabilir komutlar (ikameler dahil).
    pub commands: Vec<ParsedCommand>,
    /// Butun yonlendirmeler.
    pub redirections: Vec<Redirection>,
    /// Butun heredoc'lar.
    pub heredocs: Vec<Heredoc>,
    /// Rastlanan kabuk kontrol sozcukleri (`for`, `if`, `time`, `!` ...).
    pub control_words: Vec<String>,
    /// Tanimlanan kabuk fonksiyonlarinin adlari.
    pub function_definitions: Vec<String>,
    /// Arka plan (`&`) kullanildi mi?
    pub background: bool,
}

/// Sozcuk cozumleyicinin urettigi kelime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Word {
    /// Tirnak/kacis cozulmus duz metin (genislemeler haric).
    literal: String,
    /// Icindeki komut/surec ikamelerinin ham kaynagi.
    subs: Vec<Substitution>,
    /// Kelime herhangi bir genisleme icerdi mi?
    expanded: bool,
    /// Kelimenin herhangi bir bolumu tirnakli miydi?
    quoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Substitution {
    kind: SubKind,
    source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubKind {
    Command,
    Process,
}

impl SubKind {
    fn origin(self) -> CommandOrigin {
        match self {
            SubKind::Command => CommandOrigin::CommandSubstitution,
            SubKind::Process => CommandOrigin::ProcessSubstitution,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(Word),
    Semi,
    AndAnd,
    OrOr,
    Pipe,
    Amp,
    LParen,
    RParen,
    Newline,
    Redirect(String),
}

#[derive(Debug, Clone)]
struct PendingHeredoc {
    tag: String,
    quoted: bool,
    strip_tabs: bool,
}

/// Karakter tabanli sozcuk cozumleyici.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    tokens: Vec<Token>,
    heredocs: Vec<Heredoc>,
    pending: Vec<PendingHeredoc>,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            tokens: Vec::new(),
            heredocs: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) {
        if self.pos < self.chars.len() {
            self.pos += 1;
        }
    }

    /// Butun girdiyi tokenlara cevirir.
    fn run(mut self) -> Result<(Vec<Token>, Vec<Heredoc>), ShellParseError> {
        loop {
            self.skip_blanks();
            let Some(c) = self.peek() else { break };

            if c == '#' && self.at_word_start() {
                self.skip_to_line_end();
                continue;
            }

            match c {
                '\n' => {
                    self.bump();
                    self.tokens.push(Token::Newline);
                    self.consume_heredoc_bodies();
                }
                ';' => {
                    self.bump();
                    if self.peek() == Some(';') {
                        self.bump();
                    }
                    self.tokens.push(Token::Semi);
                }
                '(' => {
                    self.bump();
                    self.tokens.push(Token::LParen);
                }
                ')' => {
                    self.bump();
                    self.tokens.push(Token::RParen);
                }
                '|' => {
                    self.bump();
                    match self.peek() {
                        Some('|') => {
                            self.bump();
                            self.tokens.push(Token::OrOr);
                        }
                        // `|&` = stdout+stderr borusu.
                        Some('&') => {
                            self.bump();
                            self.tokens.push(Token::Pipe);
                        }
                        _ => self.tokens.push(Token::Pipe),
                    }
                }
                '&' => {
                    self.bump();
                    match self.peek() {
                        Some('&') => {
                            self.bump();
                            self.tokens.push(Token::AndAnd);
                        }
                        Some('>') => {
                            self.bump();
                            let mut op = String::from("&>");
                            if self.peek() == Some('>') {
                                self.bump();
                                op.push('>');
                            }
                            self.tokens.push(Token::Redirect(op));
                        }
                        _ => self.tokens.push(Token::Amp),
                    }
                }
                '<' | '>' => self.lex_redirect(String::new())?,
                '0'..='9' if self.digits_before_redirect() => {
                    let mut fd = String::new();
                    while let Some(d) = self.peek() {
                        if d.is_ascii_digit() {
                            fd.push(d);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.lex_redirect(fd)?;
                }
                _ => {
                    let before = self.pos;
                    let word = self.read_word()?;
                    if self.pos == before {
                        // Ilerleme yoksa sonsuz donguye girme (I6: panik yok).
                        self.bump();
                        continue;
                    }
                    self.tokens.push(Token::Word(word));
                }
            }
        }

        // Girdi heredoc govdesi acikken bitmis olabilir.
        if !self.pending.is_empty() {
            self.consume_heredoc_bodies();
        }
        Ok((self.tokens, self.heredocs))
    }

    fn skip_blanks(&mut self) {
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' || c == '\r' {
                self.bump();
            } else if c == '\\' && self.peek_at(1) == Some('\n') {
                // Satir devami.
                self.bump();
                self.bump();
            } else {
                break;
            }
        }
    }

    fn at_word_start(&self) -> bool {
        match self.pos.checked_sub(1).and_then(|i| self.chars.get(i)) {
            None => true,
            Some(prev) => matches!(prev, ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')'),
        }
    }

    fn skip_to_line_end(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Rakam dizisi hemen ardindan `<`/`>` geliyorsa bu bir fd onekidir.
    fn digits_before_redirect(&self) -> bool {
        let mut offset = 0;
        while matches!(self.peek_at(offset), Some(c) if c.is_ascii_digit()) {
            offset += 1;
        }
        offset > 0 && matches!(self.peek_at(offset), Some('<') | Some('>'))
    }

    /// `<` / `>` ile baslayan her seyi cozer: yonlendirme, heredoc, surec ikamesi.
    fn lex_redirect(&mut self, fd: String) -> Result<(), ShellParseError> {
        let Some(first) = self.peek() else {
            return Ok(());
        };
        self.bump();

        // Surec ikamesi: `<(...)` / `>(...)`
        if self.peek() == Some('(') && fd.is_empty() {
            self.bump();
            let source = self.read_balanced('(', ')', 1)?;
            let word = Word {
                literal: String::new(),
                subs: vec![Substitution {
                    kind: SubKind::Process,
                    source,
                }],
                expanded: true,
                quoted: false,
            };
            self.tokens.push(Token::Word(word));
            return Ok(());
        }

        if first == '<' && self.peek() == Some('<') {
            self.bump();
            if self.peek() == Some('<') {
                // Here-string.
                self.bump();
                self.tokens.push(Token::Redirect(format!("{fd}<<<")));
                return Ok(());
            }
            let strip_tabs = self.peek() == Some('-');
            if strip_tabs {
                self.bump();
            }
            self.skip_blanks();
            let tag_word = self.read_word()?;
            self.pending.push(PendingHeredoc {
                tag: tag_word.literal,
                quoted: tag_word.quoted,
                strip_tabs,
            });
            return Ok(());
        }

        let mut op = fd;
        op.push(first);
        match self.peek() {
            Some('>') if first == '>' => {
                self.bump();
                op.push('>');
            }
            Some('&') => {
                self.bump();
                op.push('&');
                // `2>&1` — hedef fd dogrudan operatore yapisir.
                while let Some(d) = self.peek() {
                    if d.is_ascii_digit() || d == '-' {
                        op.push(d);
                        self.bump();
                    } else {
                        break;
                    }
                }
                self.tokens.push(Token::Redirect(op));
                return Ok(());
            }
            Some('|') if first == '>' => {
                self.bump();
                op.push('|');
            }
            _ => {}
        }
        self.tokens.push(Token::Redirect(op));
        Ok(())
    }

    /// Bekleyen heredoc govdelerini girdiden yutar.
    fn consume_heredoc_bodies(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for spec in pending {
            let mut body = String::new();
            loop {
                if self.pos >= self.chars.len() {
                    break;
                }
                let mut line = String::new();
                while let Some(c) = self.peek() {
                    self.bump();
                    if c == '\n' {
                        break;
                    }
                    line.push(c);
                }
                let compared = if spec.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if compared == spec.tag {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            self.heredocs.push(Heredoc {
                tag: spec.tag,
                quoted: spec.quoted,
                strip_tabs: spec.strip_tabs,
                body,
            });
        }
    }

    /// Tek bir kelimeyi okur; tirnak ve kacislar burada cozulur.
    fn read_word(&mut self) -> Result<Word, ShellParseError> {
        let mut word = Word::default();
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\r' | '\n' | '|' | '&' | ';' | '(' | ')' | '<' | '>' => break,
                '\\' => {
                    self.bump();
                    if let Some(escaped) = self.peek() {
                        if escaped != '\n' {
                            word.literal.push(escaped);
                        }
                        self.bump();
                    }
                }
                '\'' => {
                    self.bump();
                    word.quoted = true;
                    loop {
                        match self.peek() {
                            None => return Err(ShellParseError::UnterminatedQuote),
                            Some('\'') => {
                                self.bump();
                                break;
                            }
                            Some(ch) => {
                                word.literal.push(ch);
                                self.bump();
                            }
                        }
                    }
                }
                '"' => {
                    self.bump();
                    word.quoted = true;
                    self.read_double_quoted(&mut word)?;
                }
                '$' => self.read_dollar(&mut word)?,
                '`' => {
                    self.bump();
                    let source = self.read_backtick()?;
                    word.expanded = true;
                    word.subs.push(Substitution {
                        kind: SubKind::Command,
                        source,
                    });
                }
                _ => {
                    word.literal.push(c);
                    self.bump();
                }
            }
        }
        Ok(word)
    }

    fn read_double_quoted(&mut self, word: &mut Word) -> Result<(), ShellParseError> {
        loop {
            match self.peek() {
                None => return Err(ShellParseError::UnterminatedQuote),
                Some('"') => {
                    self.bump();
                    return Ok(());
                }
                Some('\\') => {
                    self.bump();
                    if let Some(escaped) = self.peek() {
                        word.literal.push(escaped);
                        self.bump();
                    }
                }
                Some('$') => self.read_dollar(word)?,
                Some('`') => {
                    self.bump();
                    let source = self.read_backtick()?;
                    word.expanded = true;
                    word.subs.push(Substitution {
                        kind: SubKind::Command,
                        source,
                    });
                }
                Some(c) => {
                    word.literal.push(c);
                    self.bump();
                }
            }
        }
    }

    /// `$` ile baslayan her genislemeyi cozer.
    fn read_dollar(&mut self, word: &mut Word) -> Result<(), ShellParseError> {
        self.bump(); // '$'
        match self.peek() {
            Some('(') => {
                self.bump();
                if self.peek() == Some('(') {
                    // Aritmetik genisleme: komut calistirmaz.
                    self.bump();
                    let _ = self.read_balanced('(', ')', 2)?;
                    word.expanded = true;
                } else {
                    let source = self.read_balanced('(', ')', 1)?;
                    word.expanded = true;
                    word.subs.push(Substitution {
                        kind: SubKind::Command,
                        source,
                    });
                }
            }
            Some('{') => {
                self.bump();
                let inner = self.read_balanced('{', '}', 1)?;
                word.expanded = true;
                // `${x:-$(cmd)}` — parametre genislemesi icinde de komut olabilir.
                word.subs.extend(scan_expansions(&inner, 0));
            }
            Some(c) if c.is_alphanumeric() || c == '_' || c == '@' || c == '*' || c == '?' => {
                word.expanded = true;
                while let Some(n) = self.peek() {
                    if n.is_alphanumeric() || n == '_' {
                        self.bump();
                    } else {
                        self.bump();
                        break;
                    }
                }
            }
            _ => word.literal.push('$'),
        }
        Ok(())
    }

    fn read_backtick(&mut self) -> Result<String, ShellParseError> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.bump();
            if c == '\\' {
                if let Some(escaped) = self.peek() {
                    out.push(escaped);
                    self.bump();
                }
                continue;
            }
            if c == '`' {
                return Ok(out);
            }
            out.push(c);
        }
        Err(ShellParseError::UnterminatedSubstitution)
    }

    /// Acilis karakteri zaten yutulmus halde dengeli kapanisi arar; tirnaklara
    /// saygi gosterir ki `$(echo ")")` gibi metinler yanlis kapanmasin.
    fn read_balanced(
        &mut self,
        open: char,
        close: char,
        mut depth: usize,
    ) -> Result<String, ShellParseError> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.bump();
            if c == '\\' {
                out.push(c);
                if let Some(escaped) = self.peek() {
                    out.push(escaped);
                    self.bump();
                }
                continue;
            }
            if c == '\'' {
                out.push(c);
                while let Some(n) = self.peek() {
                    self.bump();
                    out.push(n);
                    if n == '\'' {
                        break;
                    }
                }
                continue;
            }
            if c == '"' {
                out.push(c);
                while let Some(n) = self.peek() {
                    self.bump();
                    out.push(n);
                    if n == '\\' {
                        if let Some(escaped) = self.peek() {
                            out.push(escaped);
                            self.bump();
                        }
                        continue;
                    }
                    if n == '"' {
                        break;
                    }
                }
                continue;
            }
            if c == open {
                depth += 1;
                out.push(c);
                continue;
            }
            if c == close {
                depth -= 1;
                if depth == 0 {
                    return Ok(out);
                }
                out.push(c);
                continue;
            }
            out.push(c);
        }
        Err(ShellParseError::UnterminatedSubstitution)
    }
}

/// Duz metin icindeki komut ikamelerini toplar (heredoc govdesi, `${...}` ici).
fn scan_expansions(text: &str, depth: usize) -> Vec<Substitution> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return out;
    }
    let mut lexer = Lexer::new(text);
    while let Some(c) = lexer.peek() {
        match c {
            '\\' => {
                lexer.bump();
                lexer.bump();
            }
            '`' => {
                lexer.bump();
                match lexer.read_backtick() {
                    Ok(source) => out.push(Substitution {
                        kind: SubKind::Command,
                        source,
                    }),
                    Err(_) => break,
                }
            }
            '$' => {
                lexer.bump();
                match lexer.peek() {
                    Some('(') => {
                        lexer.bump();
                        if lexer.peek() == Some('(') {
                            lexer.bump();
                            if lexer.read_balanced('(', ')', 2).is_err() {
                                break;
                            }
                        } else {
                            match lexer.read_balanced('(', ')', 1) {
                                Ok(source) => out.push(Substitution {
                                    kind: SubKind::Command,
                                    source,
                                }),
                                Err(_) => break,
                            }
                        }
                    }
                    Some('{') => {
                        lexer.bump();
                        match lexer.read_balanced('{', '}', 1) {
                            Ok(inner) => out.extend(scan_expansions(&inner, depth + 1)),
                            Err(_) => break,
                        }
                    }
                    _ => {}
                }
            }
            _ => lexer.bump(),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Ayristirma — komut cikarimi
// ---------------------------------------------------------------------------

/// Kabuk kontrol sozcukleri; komut konumunda gorulurse ikili sayilmaz.
const CONTROL_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "select", "function", "time", "!", "coproc",
];

/// Bir komut satirini ayristirir; ikameler ve heredoc govdeleri dahil BUTUN
/// calistirilabilir komutlari cikarir.
pub fn parse_shell(input: &str) -> Result<ShellPlan, ShellParseError> {
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(ShellParseError::TooLarge);
    }
    let mut plan = ShellPlan::default();
    parse_into(input, CommandOrigin::Direct, 0, &mut plan)?;
    Ok(plan)
}

fn parse_into(
    input: &str,
    origin: CommandOrigin,
    depth: usize,
    plan: &mut ShellPlan,
) -> Result<(), ShellParseError> {
    if depth > MAX_DEPTH {
        return Err(ShellParseError::TooDeep);
    }
    let (tokens, heredocs) = Lexer::new(input).run()?;

    let mut nested: Vec<(CommandOrigin, String)> = Vec::new();

    for heredoc in heredocs {
        if !heredoc.quoted {
            // Tirnaksiz etiket: govde genisler, icindeki komutlar CALISIR.
            for sub in scan_expansions(&heredoc.body, depth) {
                nested.push((CommandOrigin::HeredocExpansion, sub.source));
            }
        }
        plan.heredocs.push(heredoc);
    }

    let mut current = ParsedCommand {
        origin,
        depth,
        ..ParsedCommand::default()
    };
    let mut at_command_position = true;
    let mut pending_redirect: Option<String> = None;

    for token in tokens {
        match token {
            Token::Word(word) => {
                for sub in &word.subs {
                    nested.push((sub.kind.origin(), sub.source.clone()));
                }
                if let Some(op) = pending_redirect.take() {
                    plan.redirections.push(Redirection {
                        op,
                        target: word.literal.clone(),
                    });
                    continue;
                }
                if at_command_position {
                    if is_assignment(&word) {
                        current.assignments.push(word.literal);
                        continue;
                    }
                    if CONTROL_WORDS.contains(&word.literal.as_str()) {
                        plan.control_words.push(word.literal);
                        continue;
                    }
                    if word.literal == "{" || word.literal == "}" {
                        continue;
                    }
                    if word.literal.is_empty() && !word.expanded {
                        continue;
                    }
                    current.dynamic_binary = word.expanded;
                    current.path_binary = word.literal.contains('/');
                    current.binary = word.literal;
                    at_command_position = false;
                } else {
                    current.argv.push(word.literal);
                }
            }
            Token::Redirect(op) => {
                settle_redirect(&mut pending_redirect, plan);
                if redirect_is_self_contained(&op) {
                    // `2>&1` gibi bicimler hedef kelime almaz.
                    plan.redirections.push(Redirection {
                        op,
                        target: String::new(),
                    });
                } else {
                    pending_redirect = Some(op);
                }
            }
            Token::Amp => {
                settle_redirect(&mut pending_redirect, plan);
                plan.background = true;
                flush(&mut current, origin, depth, &mut at_command_position, plan)?;
            }
            Token::LParen => {
                settle_redirect(&mut pending_redirect, plan);
                // `ad() { ... }` — fonksiyon tanimi.
                if !at_command_position && current.argv.is_empty() && !current.binary.is_empty() {
                    plan.function_definitions.push(current.binary.clone());
                    current = ParsedCommand {
                        origin,
                        depth,
                        ..ParsedCommand::default()
                    };
                    at_command_position = true;
                    continue;
                }
                flush(&mut current, origin, depth, &mut at_command_position, plan)?;
            }
            Token::Semi
            | Token::AndAnd
            | Token::OrOr
            | Token::Pipe
            | Token::Newline
            | Token::RParen => {
                settle_redirect(&mut pending_redirect, plan);
                flush(&mut current, origin, depth, &mut at_command_position, plan)?;
            }
        }
    }

    if pending_redirect.is_some() {
        return Err(ShellParseError::DanglingRedirect);
    }
    flush(&mut current, origin, depth, &mut at_command_position, plan)?;

    for (nested_origin, source) in nested {
        parse_into(&source, nested_origin, depth + 1, plan)?;
    }
    Ok(())
}

/// Hedef kelimesi gelmeyen bekleyen yonlendirmeyi kayda gecirir.
fn settle_redirect(pending: &mut Option<String>, plan: &mut ShellPlan) {
    if let Some(op) = pending.take() {
        plan.redirections.push(Redirection {
            op,
            target: String::new(),
        });
    }
}

/// `2>&1`, `>&2`, `<&-` gibi bicimler hedef kelime almaz.
fn redirect_is_self_contained(op: &str) -> bool {
    let rest = op.trim_start_matches(|c: char| c.is_ascii_digit());
    match rest.strip_prefix(">&").or_else(|| rest.strip_prefix("<&")) {
        Some(tail) => !tail.is_empty(),
        None => false,
    }
}

fn flush(
    current: &mut ParsedCommand,
    origin: CommandOrigin,
    depth: usize,
    at_command_position: &mut bool,
    plan: &mut ShellPlan,
) -> Result<(), ShellParseError> {
    let finished = std::mem::replace(
        current,
        ParsedCommand {
            origin,
            depth,
            ..ParsedCommand::default()
        },
    );
    *at_command_position = true;
    if finished.binary.is_empty() && finished.assignments.is_empty() && !finished.dynamic_binary {
        return Ok(());
    }
    if plan.commands.len() >= MAX_COMMANDS {
        return Err(ShellParseError::TooLarge);
    }
    plan.commands.push(finished);
    Ok(())
}

/// `VAR=deger` / `VAR+=deger` bicimi mi?
fn is_assignment(word: &Word) -> bool {
    if word.quoted {
        return false;
    }
    let Some(eq) = word.literal.find('=') else {
        return false;
    };
    let name = &word.literal[..eq];
    let name = name.strip_suffix('+').unwrap_or(name);
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Politika
// ---------------------------------------------------------------------------

/// Red kurallari — her red bir kurala baglanir, denetimde ayirt edilir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyRule {
    /// Komut satiri ayristirilamadi.
    ParseError,
    /// Calistirilabilir komut yok / yalniz ortam atamasi var.
    EmptyCommand,
    /// Ikili adi calisma aninda genisliyor.
    DynamicBinary,
    /// Ikili yol ile cagriliyor.
    PathBinary,
    /// Ikili kalici red listesinde (yorumlayici / calistirma sarmalayicisi).
    NeverAllowed,
    /// Ikili izin listesinde degil.
    NotAllowlisted,
    /// Izinli ikili yasak arguman tasiyor.
    ArgumentRule,
    /// Komut onunde ortam degiskeni atamasi var.
    EnvAssignment,
    /// Yonlendirme kullanildi.
    Redirection,
    /// Heredoc kullanildi.
    Heredoc,
    /// Arka plan calistirmasi istendi.
    Background,
    /// Kabuk kontrol akisi yapisi kullanildi.
    ControlFlow,
    /// Kabuk fonksiyonu tanimlandi.
    FunctionDefinition,
}

impl DenyRule {
    /// Denetim kaydina yazilan sabit etiket.
    pub fn as_str(&self) -> &'static str {
        match self {
            DenyRule::ParseError => "parse_error",
            DenyRule::EmptyCommand => "empty_command",
            DenyRule::DynamicBinary => "dynamic_binary",
            DenyRule::PathBinary => "path_binary",
            DenyRule::NeverAllowed => "never_allowed",
            DenyRule::NotAllowlisted => "not_allowlisted",
            DenyRule::ArgumentRule => "argument_rule",
            DenyRule::EnvAssignment => "env_assignment",
            DenyRule::Redirection => "redirection",
            DenyRule::Heredoc => "heredoc",
            DenyRule::Background => "background",
            DenyRule::ControlFlow => "control_flow",
            DenyRule::FunctionDefinition => "function_definition",
        }
    }
}

/// Tek bir red gerekcesi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// Ihlal edilen kural.
    pub rule: DenyRule,
    /// Redde konu olan metin (ikili adi, operator, etiket ...).
    pub target: String,
    /// Insan okunur gerekce.
    pub detail: String,
    /// Redde konu komutun kaynagi.
    pub origin: CommandOrigin,
}

impl Denial {
    fn new(
        rule: DenyRule,
        target: impl Into<String>,
        detail: impl Into<String>,
        origin: CommandOrigin,
    ) -> Self {
        Self {
            rule,
            target: target.into(),
            detail: detail.into(),
            origin,
        }
    }

    /// Denetim kaydina yazilan tek satirlik gerekce.
    pub fn reason(&self) -> String {
        format!(
            "[{}/{}] '{}': {}",
            self.rule.as_str(),
            self.origin.as_str(),
            self.target,
            self.detail
        )
    }
}

/// Politika karari.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellVerdict {
    /// Komut satirinin tamami izinli.
    Allow,
    /// En az bir kural ihlal edildi; TUM ihlaller tasinir.
    Deny(Vec<Denial>),
}

impl ShellVerdict {
    /// Karar izin veriyor mu?
    pub fn is_allowed(&self) -> bool {
        matches!(self, ShellVerdict::Allow)
    }

    /// Red gerekceleri; izinli kararda bos dilim.
    pub fn denials(&self) -> &[Denial] {
        match self {
            ShellVerdict::Allow => &[],
            ShellVerdict::Deny(denials) => denials,
        }
    }

    /// Butun gerekceleri tek satirda birlestirir.
    pub fn reason(&self) -> Option<String> {
        match self {
            ShellVerdict::Allow => None,
            ShellVerdict::Deny(denials) => Some(
                denials
                    .iter()
                    .map(Denial::reason)
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
        }
    }
}

/// Izinli bir ikilinin arguman kisitlari.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentRule {
    /// Kuralin bagli oldugu ikili.
    pub binary: String,
    /// Yasak bayraklar (`-c`, `--config` ...).
    pub denied_flags: Vec<String>,
    /// Yasak alt komutlar.
    pub denied_subcommands: Vec<String>,
    /// Bos degilse: yalniz bu alt komutlar gecer.
    pub allowed_subcommands: Vec<String>,
}

impl ArgumentRule {
    /// Yalniz ikili adiyla bos kural.
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            denied_flags: Vec::new(),
            denied_subcommands: Vec::new(),
            allowed_subcommands: Vec::new(),
        }
    }

    /// Yasak bayraklari baglar.
    pub fn deny_flags<I, S>(mut self, flags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.denied_flags.extend(flags.into_iter().map(Into::into));
        self
    }

    /// Yasak alt komutlari baglar.
    pub fn deny_subcommands<I, S>(mut self, subs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.denied_subcommands
            .extend(subs.into_iter().map(Into::into));
        self
    }

    /// Izinli alt komut kumesini baglar.
    pub fn allow_subcommands<I, S>(mut self, subs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_subcommands
            .extend(subs.into_iter().map(Into::into));
        self
    }
}

/// Kalici red listesi: yorumlayicilar ve calistirma sarmalayicilari.
///
/// Bu adlar izin listesine EKLENEMEZ — biri izinli olsa "gecici serbest shell"
/// kalici hale gelirdi. Yapilandirma hatasina karsi ikinci savunma katmanidir.
const NEVER_ALLOWED: &[&str] = &[
    // Kabuklar ve `-c` ile keyfi metin calistiranlar.
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "fish",
    "busybox",
    "command",
    "builtin",
    "eval",
    "exec",
    "source",
    ".",
    // Baska komutu calistiran sarmalayicilar.
    "env",
    "xargs",
    "find",
    "nice",
    "nohup",
    "setsid",
    "timeout",
    "time",
    "watch",
    "parallel",
    "script",
    "stdbuf",
    "chroot",
    "sudo",
    "doas",
    "su",
    "ssh",
    "socat",
    "nc",
    "ncat",
    "telnet",
    "make",
    "gmake",
    "docker",
    "podman",
    "kubectl",
    // 9.1'de acikca sayilan metin araclari (retrieval yolu native tool'dur).
    "grep",
    "egrep",
    "fgrep",
    "rgrep",
    "rg",
    "ag",
    "ack",
    "sed",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "cat",
    "tac",
    "head",
    "tail",
    "cut",
    "tr",
    "tee",
    "dd",
    // Genel amacli yorumlayicilar.
    "perl",
    "python",
    "python2",
    "python3",
    "ruby",
    "node",
    "deno",
    "bun",
    "php",
    "lua",
    "tclsh",
    "expect",
    "osascript",
];

/// Faz 5 varsayilan izin listesi (MASTER-PLAN 9.1).
const DEFAULT_ALLOWED: &[&str] = &["cargo", "git", "npm", "pytest"];

/// Shell politikasi — TEK karar noktasi.
#[derive(Debug, Clone)]
pub struct ShellPolicy {
    allowed: Vec<String>,
    denied: Vec<String>,
    argument_rules: Vec<ArgumentRule>,
    allow_redirection: bool,
    allow_heredoc: bool,
    allow_background: bool,
    allow_control_flow: bool,
    allow_env_assignment: bool,
}

impl Default for ShellPolicy {
    fn default() -> Self {
        Self {
            allowed: DEFAULT_ALLOWED.iter().map(|s| s.to_string()).collect(),
            denied: NEVER_ALLOWED.iter().map(|s| s.to_string()).collect(),
            argument_rules: default_argument_rules(),
            allow_redirection: false,
            allow_heredoc: false,
            allow_background: false,
            allow_control_flow: false,
            allow_env_assignment: false,
        }
    }
}

/// Izinli ikililerin bilinen kacis argumanlari.
fn default_argument_rules() -> Vec<ArgumentRule> {
    vec![
        // `git -c core.pager=...` / `git -c alias.x=!cmd` keyfi ikili calistirir.
        ArgumentRule::new("git")
            .deny_flags([
                "-c",
                "--config-env",
                "--exec-path",
                "--upload-pack",
                "--receive-pack",
                "-p",
                "--paginate",
            ])
            .deny_subcommands(["config", "filter-branch", "daemon", "instaweb", "submodule"]),
        // `cargo --config target.x.runner=...` build kosucusunu degistirir.
        ArgumentRule::new("cargo")
            .deny_flags(["--config", "-Z"])
            .deny_subcommands(["install", "publish", "login", "owner", "yank"]),
        // `npm run <script>` package.json'daki keyfi kabuk satirini calistirir.
        ArgumentRule::new("npm")
            .deny_flags(["--node-options", "--script-shell", "--ignore-scripts=false"])
            .deny_subcommands(["run", "run-script", "exec", "x", "start", "publish", "link"])
            .allow_subcommands(["ci", "install", "i", "test", "ls", "audit", "outdated", "view"]),
        // `pytest -p modul` keyfi eklenti yukler.
        ArgumentRule::new("pytest").deny_flags(["-p", "--pdb", "--pdbcls"]),
    ]
}

impl ShellPolicy {
    /// Varsayilan Faz 5 politikasi.
    pub fn new() -> Self {
        Self::default()
    }

    /// Izin listesini tumden degistirir; kalici red listesi baskin kalir.
    pub fn with_allowed<I, S>(mut self, binaries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed = binaries
            .into_iter()
            .map(Into::into)
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .collect();
        self.allowed.dedup();
        self
    }

    /// Izin listesine tek ad ekler.
    pub fn allow_binary(mut self, binary: impl Into<String>) -> Self {
        let binary = binary.into().trim().to_string();
        if !binary.is_empty() && !self.allowed.contains(&binary) {
            self.allowed.push(binary);
        }
        self
    }

    /// Red listesine tek ad ekler.
    pub fn deny_binary(mut self, binary: impl Into<String>) -> Self {
        let binary = binary.into().trim().to_string();
        if !binary.is_empty() && !self.denied.contains(&binary) {
            self.denied.push(binary);
        }
        self
    }

    /// Arguman kurali ekler / var olani degistirir.
    pub fn with_argument_rule(mut self, rule: ArgumentRule) -> Self {
        self.argument_rules.retain(|r| r.binary != rule.binary);
        self.argument_rules.push(rule);
        self
    }

    /// Izin listesi (kalici red listesi cikarilmis halde).
    pub fn allowed_binaries(&self) -> Vec<&str> {
        self.allowed
            .iter()
            .map(String::as_str)
            .filter(|b| !self.is_denied(b))
            .collect()
    }

    /// Kalici red listesi.
    pub fn denied_binaries(&self) -> Vec<&str> {
        self.denied.iter().map(String::as_str).collect()
    }

    fn is_denied(&self, binary: &str) -> bool {
        self.denied.iter().any(|b| b == binary)
    }

    fn is_allowed(&self, binary: &str) -> bool {
        self.allowed.iter().any(|b| b == binary)
    }

    /// Ayristirilmis plani degerlendirir.
    pub fn evaluate_plan(&self, plan: &ShellPlan) -> ShellVerdict {
        let mut denials = Vec::new();

        if !self.allow_redirection {
            for redirection in &plan.redirections {
                denials.push(Denial::new(
                    DenyRule::Redirection,
                    format!("{} {}", redirection.op, redirection.target),
                    "yonlendirme reddedilir; dosya yazimi fs-shim uzerinden gecer",
                    CommandOrigin::Direct,
                ));
            }
        }
        if !self.allow_heredoc {
            for heredoc in &plan.heredocs {
                denials.push(Denial::new(
                    DenyRule::Heredoc,
                    heredoc.tag.clone(),
                    "heredoc reddedilir; govde denetlenmeyen girdi kanalidir",
                    CommandOrigin::Direct,
                ));
            }
        }
        if !self.allow_background && plan.background {
            denials.push(Denial::new(
                DenyRule::Background,
                "&",
                "arka plan calistirmasi reddedilir; surec tur dongusunden kacar",
                CommandOrigin::Direct,
            ));
        }
        if !self.allow_control_flow {
            for word in &plan.control_words {
                denials.push(Denial::new(
                    DenyRule::ControlFlow,
                    word.clone(),
                    "kabuk kontrol yapisi reddedilir",
                    CommandOrigin::Direct,
                ));
            }
        }
        for name in &plan.function_definitions {
            denials.push(Denial::new(
                DenyRule::FunctionDefinition,
                name.clone(),
                "kabuk fonksiyon tanimi reddedilir",
                CommandOrigin::Direct,
            ));
        }

        if plan.commands.is_empty() {
            denials.push(Denial::new(
                DenyRule::EmptyCommand,
                "",
                "calistirilabilir komut bulunamadi",
                CommandOrigin::Direct,
            ));
        }

        for command in &plan.commands {
            self.check_command(command, &mut denials);
        }

        if denials.is_empty() {
            ShellVerdict::Allow
        } else {
            ShellVerdict::Deny(denials)
        }
    }

    /// Ham komut satirini ayristirip degerlendirir. Ayristirma hatasi = RED.
    pub fn evaluate(&self, command_line: &str) -> ShellVerdict {
        match parse_shell(command_line) {
            Ok(plan) => self.evaluate_plan(&plan),
            Err(err) => ShellVerdict::Deny(vec![Denial::new(
                DenyRule::ParseError,
                truncate(command_line),
                format!("ayristirilamadi: {err}"),
                CommandOrigin::Direct,
            )]),
        }
    }

    fn check_command(&self, command: &ParsedCommand, denials: &mut Vec<Denial>) {
        if !self.allow_env_assignment && !command.assignments.is_empty() {
            for assignment in &command.assignments {
                denials.push(Denial::new(
                    DenyRule::EnvAssignment,
                    assignment.clone(),
                    "komut onunde ortam atamasi reddedilir; ortam tek noktadan verilir",
                    command.origin,
                ));
            }
        }

        if command.dynamic_binary {
            denials.push(Denial::new(
                DenyRule::DynamicBinary,
                truncate(&command.binary),
                "ikili adi calisma aninda genisliyor; statik olarak denetlenemez",
                command.origin,
            ));
            return;
        }

        if command.binary.is_empty() {
            if !command.assignments.is_empty() {
                denials.push(Denial::new(
                    DenyRule::EmptyCommand,
                    command.assignments.join(" "),
                    "komutsuz ortam atamasi reddedilir",
                    command.origin,
                ));
            }
            return;
        }

        if command.path_binary {
            denials.push(Denial::new(
                DenyRule::PathBinary,
                command.binary.clone(),
                "ikili yol ile cagrilamaz; yalniz izin listesindeki cikplak ad",
                command.origin,
            ));
            return;
        }

        if self.is_denied(&command.binary) {
            denials.push(Denial::new(
                DenyRule::NeverAllowed,
                command.binary.clone(),
                "kalici red listesinde (yorumlayici / calistirma sarmalayicisi)",
                command.origin,
            ));
            return;
        }

        if !self.is_allowed(&command.binary) {
            denials.push(Denial::new(
                DenyRule::NotAllowlisted,
                command.binary.clone(),
                "izin listesinde degil",
                command.origin,
            ));
            return;
        }

        self.check_arguments(command, denials);
    }

    fn check_arguments(&self, command: &ParsedCommand, denials: &mut Vec<Denial>) {
        let Some(rule) = self
            .argument_rules
            .iter()
            .find(|r| r.binary == command.binary)
        else {
            return;
        };

        for arg in &command.argv {
            for flag in &rule.denied_flags {
                if flag_matches(arg, flag) {
                    denials.push(Denial::new(
                        DenyRule::ArgumentRule,
                        format!("{} {}", command.binary, arg),
                        format!("'{flag}' bayragi keyfi calistirmaya acilir"),
                        command.origin,
                    ));
                }
            }
        }

        let subcommand = command.argv.iter().find(|arg| !arg.starts_with('-'));
        let Some(subcommand) = subcommand else {
            return;
        };
        if rule.denied_subcommands.iter().any(|s| s == subcommand) {
            denials.push(Denial::new(
                DenyRule::ArgumentRule,
                format!("{} {}", command.binary, subcommand),
                "alt komut yasak listesinde",
                command.origin,
            ));
            return;
        }
        if !rule.allowed_subcommands.is_empty()
            && !rule.allowed_subcommands.iter().any(|s| s == subcommand)
        {
            denials.push(Denial::new(
                DenyRule::ArgumentRule,
                format!("{} {}", command.binary, subcommand),
                "alt komut izin listesinde degil",
                command.origin,
            ));
        }
    }
}

/// Bayrak eslesmesi: tam ad, `bayrak=deger` veya bitisik kisa bayrak.
fn flag_matches(arg: &str, flag: &str) -> bool {
    if arg == flag {
        return true;
    }
    if arg
        .strip_prefix(flag)
        .is_some_and(|rest| rest.starts_with('='))
    {
        return true;
    }
    let short = flag.len() == 2 && flag.starts_with('-') && !flag.starts_with("--");
    short && arg.starts_with(flag) && arg.len() > flag.len()
}

/// Denetim kaydina yazilan metni kirpar.
fn truncate(text: &str) -> String {
    let mut out: String = text.chars().take(AUDIT_TARGET_LIMIT).collect();
    if text.chars().count() > AUDIT_TARGET_LIMIT {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------------
// Acil kacis (K3)
// ---------------------------------------------------------------------------

/// `interrupts` satirinin bellek karsiligi (migrations/0005).
///
/// Yazim omni-storage'in isidir; bu modul kaydi uretir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptRecord {
    /// Kesintiye konu ajan.
    pub agent_id: i64,
    /// `interrupts.kind`.
    pub kind: String,
    /// `interrupts.source`.
    pub source: String,
    /// Kaydin acilis zamani (RFC 3339).
    pub ts: String,
    /// Kapanis zamani; acik kayitta `None`.
    pub resolved_at: Option<String>,
}

/// Ajanin actigi acil kacis talebi — GEREKCE zorunludur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeRequest {
    /// Talebi acan ajan.
    pub agent_id: i64,
    /// Native tool'un neden yetmedigi.
    pub justification: String,
    /// Calistirilmak istenen komut satiri.
    pub command_line: String,
    /// Istenen sure (saniye).
    pub requested_secs: i64,
    /// Talebin zamani.
    pub ts: DateTime<Utc>,
}

impl EscapeRequest {
    /// Talep uretir; zaman damgasi uretim aninda konur.
    pub fn new(
        agent_id: i64,
        justification: impl Into<String>,
        command_line: impl Into<String>,
        requested_secs: i64,
    ) -> Self {
        Self::at(
            agent_id,
            justification,
            command_line,
            requested_secs,
            Utc::now(),
        )
    }

    /// Zamani disaridan verilen talep (test edilebilirlik).
    pub fn at(
        agent_id: i64,
        justification: impl Into<String>,
        command_line: impl Into<String>,
        requested_secs: i64,
        ts: DateTime<Utc>,
    ) -> Self {
        Self {
            agent_id,
            justification: justification.into(),
            command_line: command_line.into(),
            requested_secs,
            ts,
        }
    }

    fn validate(&self) -> Result<(), ToolsError> {
        if self.justification.trim().chars().count() < MIN_JUSTIFICATION_CHARS {
            return Err(ToolsError::Denied(format!(
                "acil kacis talebi gerekcesiz: en az {MIN_JUSTIFICATION_CHARS} karakter gerekir"
            )));
        }
        if self.command_line.trim().is_empty() {
            return Err(ToolsError::Denied(
                "acil kacis talebinde komut satiri bos".to_string(),
            ));
        }
        if self.requested_secs <= 0 {
            return Err(ToolsError::Denied(
                "acil kacis suresi pozitif olmali".to_string(),
            ));
        }
        Ok(())
    }
}

/// Yargicin verdigi SURELI izin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeGrant {
    /// Izinli ajan.
    pub agent_id: i64,
    /// Onaylayan merci (`capability_audit.approver`).
    pub approver: String,
    /// Talebin gerekcesi (kayit icin tasinir).
    pub justification: String,
    /// Iznin baslangici.
    pub granted_at: DateTime<Utc>,
    /// Iznin bitisi — bu andan sonra politika yeniden zorlanir.
    pub expires_at: DateTime<Utc>,
}

impl EscapeGrant {
    /// Verilen anda izin hala gecerli mi?
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        now >= self.granted_at && now < self.expires_at
    }

    /// Denetim kaydina yazilan ozet.
    pub fn summary(&self) -> String {
        format!(
            "acil kacis izni: onaylayan={} bitis={}",
            self.approver,
            self.expires_at.to_rfc3339()
        )
    }
}

/// Acil kacis defteri: bekleyen talepler + gecerli izinler.
#[derive(Debug, Clone)]
pub struct EscapeLedger {
    pending: HashMap<i64, EscapeRequest>,
    grants: HashMap<i64, EscapeGrant>,
    max_ttl_secs: i64,
}

impl Default for EscapeLedger {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            grants: HashMap::new(),
            max_ttl_secs: DEFAULT_MAX_ESCAPE_SECS,
        }
    }
}

impl EscapeLedger {
    /// Varsayilan ust sureli defter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Izin ust suresini degistirir (saniye, pozitif olmali).
    pub fn with_max_ttl_secs(mut self, secs: i64) -> Self {
        if secs > 0 {
            self.max_ttl_secs = secs;
        }
        self
    }

    /// Izin ust suresi.
    pub fn max_ttl_secs(&self) -> i64 {
        self.max_ttl_secs
    }

    /// Talebi acar; `interrupts` satiri doner. Bu adim IZIN VERMEZ.
    pub fn open(&mut self, request: EscapeRequest) -> Result<InterruptRecord, ToolsError> {
        request.validate()?;
        let record = InterruptRecord {
            agent_id: request.agent_id,
            kind: INTERRUPT_KIND_SHELL_ESCAPE.to_string(),
            source: INTERRUPT_SOURCE_AGENT.to_string(),
            ts: request.ts.to_rfc3339(),
            resolved_at: None,
        };
        self.pending.insert(request.agent_id, request);
        Ok(record)
    }

    /// Bekleyen talep.
    pub fn pending(&self, agent_id: i64) -> Option<&EscapeRequest> {
        self.pending.get(&agent_id)
    }

    /// YARGIC onayi: bekleyen talebi sureli izne cevirir.
    pub fn approve(
        &mut self,
        agent_id: i64,
        approver: impl Into<String>,
        ttl_secs: i64,
        now: DateTime<Utc>,
    ) -> Result<(EscapeGrant, InterruptRecord), ToolsError> {
        let Some(request) = self.pending.remove(&agent_id) else {
            return Err(ToolsError::Denied(format!(
                "ajan {agent_id} icin bekleyen acil kacis talebi yok"
            )));
        };
        let approver = approver.into();
        if approver.trim().is_empty() {
            self.pending.insert(agent_id, request);
            return Err(ToolsError::Denied(
                "acil kacis onayi icin merci adi gerekir".to_string(),
            ));
        }
        if ttl_secs <= 0 {
            self.pending.insert(agent_id, request);
            return Err(ToolsError::Denied(
                "acil kacis suresi pozitif olmali".to_string(),
            ));
        }
        let ttl = ttl_secs.min(self.max_ttl_secs).min(request.requested_secs);
        let Some(window) = Duration::try_seconds(ttl) else {
            self.pending.insert(agent_id, request);
            return Err(ToolsError::Denied(
                "acil kacis suresi cozumlenemedi".to_string(),
            ));
        };
        let grant = EscapeGrant {
            agent_id,
            approver,
            justification: request.justification,
            granted_at: now,
            expires_at: now + window,
        };
        let record = InterruptRecord {
            agent_id,
            kind: INTERRUPT_KIND_SHELL_ESCAPE.to_string(),
            source: INTERRUPT_SOURCE_JUDGE.to_string(),
            ts: request.ts.to_rfc3339(),
            resolved_at: Some(now.to_rfc3339()),
        };
        self.grants.insert(agent_id, grant.clone());
        Ok((grant, record))
    }

    /// YARGIC reddi: talep kapanir, izin dogmaz.
    pub fn reject(
        &mut self,
        agent_id: i64,
        now: DateTime<Utc>,
    ) -> Result<InterruptRecord, ToolsError> {
        let Some(request) = self.pending.remove(&agent_id) else {
            return Err(ToolsError::Denied(format!(
                "ajan {agent_id} icin bekleyen acil kacis talebi yok"
            )));
        };
        Ok(InterruptRecord {
            agent_id,
            kind: INTERRUPT_KIND_SHELL_ESCAPE.to_string(),
            source: INTERRUPT_SOURCE_JUDGE.to_string(),
            ts: request.ts.to_rfc3339(),
            resolved_at: Some(now.to_rfc3339()),
        })
    }

    /// Verilen anda gecerli izin.
    pub fn active_grant(&self, agent_id: i64, now: DateTime<Utc>) -> Option<&EscapeGrant> {
        self.grants
            .get(&agent_id)
            .filter(|grant| grant.is_active_at(now))
    }

    /// Izni suresi dolmadan geri alir.
    pub fn revoke(&mut self, agent_id: i64) -> bool {
        self.grants.remove(&agent_id).is_some()
    }

    /// Suresi dolmus izinleri defterden atar.
    pub fn purge_expired(&mut self, now: DateTime<Utc>) {
        self.grants.retain(|_, grant| grant.is_active_at(now));
    }
}

// ---------------------------------------------------------------------------
// Zorlama gecidi
// ---------------------------------------------------------------------------

/// Bir shell cagrisinin akibeti.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellOutcome {
    /// Politika izin verdi.
    Allowed,
    /// Politika reddetti ama yargicin sureli izni gecerli.
    AllowedByEscape(Box<EscapeGrant>),
    /// Reddedildi.
    Denied(Vec<Denial>),
}

impl ShellOutcome {
    /// Cagri calisabilir mi?
    pub fn is_allowed(&self) -> bool {
        matches!(
            self,
            ShellOutcome::Allowed | ShellOutcome::AllowedByEscape(_)
        )
    }

    /// Red gerekceleri.
    pub fn denials(&self) -> &[Denial] {
        match self {
            ShellOutcome::Denied(denials) => denials,
            _ => &[],
        }
    }
}

/// Shell zorlama gecidi: politika + acil kacis defteri + denetim kaydi.
#[derive(Debug, Clone, Default)]
pub struct ShellGuard {
    policy: ShellPolicy,
    ledger: EscapeLedger,
}

impl ShellGuard {
    /// Verilen politikayla gecit kurar.
    pub fn new(policy: ShellPolicy) -> Self {
        Self {
            policy,
            ledger: EscapeLedger::new(),
        }
    }

    /// Defteri disaridan verir (ust sure ayari icin).
    pub fn with_ledger(mut self, ledger: EscapeLedger) -> Self {
        self.ledger = ledger;
        self
    }

    /// Politika.
    pub fn policy(&self) -> &ShellPolicy {
        &self.policy
    }

    /// Acil kacis defteri.
    pub fn ledger(&self) -> &EscapeLedger {
        &self.ledger
    }

    /// Yalniz politika karari — acil kacis DIKKATE ALINMAZ.
    pub fn evaluate(&self, command_line: &str) -> ShellVerdict {
        self.policy.evaluate(command_line)
    }

    /// Politika + acil kacis karari.
    pub fn evaluate_at(
        &self,
        agent_id: i64,
        command_line: &str,
        now: DateTime<Utc>,
    ) -> ShellOutcome {
        match self.policy.evaluate(command_line) {
            ShellVerdict::Allow => ShellOutcome::Allowed,
            ShellVerdict::Deny(denials) => match self.ledger.active_grant(agent_id, now) {
                Some(grant) => ShellOutcome::AllowedByEscape(Box::new(grant.clone())),
                None => ShellOutcome::Denied(denials),
            },
        }
    }

    /// TEK zorlama noktasi: karar verir, denetime yazar, reddi hata dondurur.
    ///
    /// Red durumunda hem `capability_audit` hem `tool_calls` satiri uretilir —
    /// denenen ve reddedilen her komut loglanir (K3 kabul kapisi).
    pub fn authorize(
        &self,
        gate: &AuditGate,
        agent_id: i64,
        command_line: &str,
    ) -> Result<(), ToolsError> {
        self.authorize_at(gate, agent_id, command_line, Utc::now())
    }

    /// Zamani disaridan verilen zorlama (test edilebilirlik).
    pub fn authorize_at(
        &self,
        gate: &AuditGate,
        agent_id: i64,
        command_line: &str,
        now: DateTime<Utc>,
    ) -> Result<(), ToolsError> {
        let target = truncate(command_line);
        match self.evaluate_at(agent_id, command_line, now) {
            ShellOutcome::Allowed => {
                gate.emit_capability(CapabilityDecision::new(
                    agent_id,
                    CAPABILITY_SHELL,
                    target,
                    Decision::Allow,
                ));
                Ok(())
            }
            ShellOutcome::AllowedByEscape(grant) => {
                tracing::warn!(
                    agent_id,
                    approver = %grant.approver,
                    expires_at = %grant.expires_at.to_rfc3339(),
                    "shell komutu acil kacis izniyle gecti"
                );
                gate.emit_capability(
                    CapabilityDecision::new(
                        agent_id,
                        CAPABILITY_SHELL_ESCAPE,
                        target,
                        Decision::Allow,
                    )
                    .with_approver(grant.approver.clone()),
                );
                Ok(())
            }
            ShellOutcome::Denied(denials) => {
                let reason = denials
                    .iter()
                    .map(Denial::reason)
                    .collect::<Vec<_>>()
                    .join("; ");
                tracing::warn!(agent_id, reason = %reason, "shell komutu reddedildi");
                gate.emit_capability(CapabilityDecision::new(
                    agent_id,
                    CAPABILITY_SHELL,
                    target,
                    Decision::Deny(reason.clone()),
                ));
                gate.emit_tool_call(
                    ToolCallRecord::denied(agent_id, SHELL_TOOL)
                        .with_args(Some(truncate(command_line))),
                );
                Err(ToolsError::Denied(reason))
            }
        }
    }

    /// Ajan acil kacis ister: gerekce + komut denetime ve `interrupts`'a duser.
    ///
    /// Bu cagri IZIN VERMEZ; yargic onayina kadar politika zorlanmaya devam eder.
    pub fn request_escape(
        &mut self,
        gate: &AuditGate,
        request: EscapeRequest,
    ) -> Result<InterruptRecord, ToolsError> {
        let agent_id = request.agent_id;
        let target = truncate(&request.command_line);
        let justification = request.justification.clone();
        match self.ledger.open(request) {
            Ok(record) => {
                gate.emit_capability(CapabilityDecision::new(
                    agent_id,
                    CAPABILITY_SHELL_ESCAPE,
                    target,
                    Decision::Deny(format!("acil kacis talebi onay bekliyor: {justification}")),
                ));
                Ok(record)
            }
            Err(err) => {
                gate.emit_capability(CapabilityDecision::new(
                    agent_id,
                    CAPABILITY_SHELL_ESCAPE,
                    target,
                    Decision::Deny(err.to_string()),
                ));
                Err(err)
            }
        }
    }

    /// YARGIC onayi: sureli izin dogar, denetime onaylayanla birlikte yazilir.
    pub fn approve_escape(
        &mut self,
        gate: &AuditGate,
        agent_id: i64,
        approver: impl Into<String>,
        ttl_secs: i64,
    ) -> Result<(EscapeGrant, InterruptRecord), ToolsError> {
        self.approve_escape_at(gate, agent_id, approver, ttl_secs, Utc::now())
    }

    /// Zamani disaridan verilen onay.
    pub fn approve_escape_at(
        &mut self,
        gate: &AuditGate,
        agent_id: i64,
        approver: impl Into<String>,
        ttl_secs: i64,
        now: DateTime<Utc>,
    ) -> Result<(EscapeGrant, InterruptRecord), ToolsError> {
        let approver = approver.into();
        let target = self
            .ledger
            .pending(agent_id)
            .map(|req| truncate(&req.command_line))
            .unwrap_or_default();
        match self.ledger.approve(agent_id, approver.clone(), ttl_secs, now) {
            Ok((grant, record)) => {
                gate.emit_capability(
                    CapabilityDecision::new(
                        agent_id,
                        CAPABILITY_SHELL_ESCAPE,
                        target,
                        Decision::Allow,
                    )
                    .with_approver(grant.approver.clone()),
                );
                Ok((grant, record))
            }
            Err(err) => {
                gate.emit_capability(
                    CapabilityDecision::new(
                        agent_id,
                        CAPABILITY_SHELL_ESCAPE,
                        target,
                        Decision::Deny(err.to_string()),
                    )
                    .with_approver(approver),
                );
                Err(err)
            }
        }
    }

    /// YARGIC reddi: talep kapanir, izin dogmaz.
    pub fn reject_escape_at(
        &mut self,
        gate: &AuditGate,
        agent_id: i64,
        approver: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<InterruptRecord, ToolsError> {
        let approver = approver.into();
        let target = self
            .ledger
            .pending(agent_id)
            .map(|req| truncate(&req.command_line))
            .unwrap_or_default();
        let record = self.ledger.reject(agent_id, now)?;
        gate.emit_capability(
            CapabilityDecision::new(
                agent_id,
                CAPABILITY_SHELL_ESCAPE,
                target,
                Decision::Deny("yargic acil kacisi reddetti".to_string()),
            )
            .with_approver(approver),
        );
        Ok(record)
    }

    /// Izni suresi dolmadan geri alir.
    pub fn revoke_escape(&mut self, agent_id: i64) -> bool {
        self.ledger.revoke(agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::{AuditEvent, audit_channel};

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_else(Utc::now)
    }

    fn binaries(plan: &ShellPlan) -> Vec<String> {
        plan.commands.iter().map(|c| c.binary.clone()).collect()
    }

    fn parse(input: &str) -> ShellPlan {
        match parse_shell(input) {
            Ok(plan) => plan,
            Err(err) => panic!("ayristirma basarisiz ({input:?}): {err}"),
        }
    }

    #[test]
    fn boru_hatti_her_ikiliyi_cikarir() {
        let plan = parse("cargo test | grep foo | sed -n 1p");
        assert_eq!(binaries(&plan), ["cargo", "grep", "sed"]);
    }

    #[test]
    fn zincirler_ayristirilir() {
        let plan = parse("cargo build && git status ; npm ci || cat x");
        assert_eq!(binaries(&plan), ["cargo", "git", "npm", "cat"]);
    }

    #[test]
    fn komut_ikamesi_ic_komutu_cikarir() {
        let plan = parse("echo $(cat /etc/passwd)");
        assert_eq!(binaries(&plan), ["echo", "cat"]);
        assert_eq!(plan.commands[1].origin, CommandOrigin::CommandSubstitution);
    }

    #[test]
    fn backtick_ikamesi_cikarilir() {
        let plan = parse("echo `id`");
        assert_eq!(binaries(&plan), ["echo", "id"]);
    }

    #[test]
    fn ic_ice_ikame_cozulur() {
        let plan = parse("echo $(echo $(cat x))");
        assert_eq!(binaries(&plan), ["echo", "echo", "cat"]);
    }

    #[test]
    fn surec_ikamesi_cikarilir() {
        let plan = parse("diff <(cat a) <(cat b)");
        assert_eq!(binaries(&plan), ["diff", "cat", "cat"]);
        assert!(
            plan.commands
                .iter()
                .filter(|c| c.origin == CommandOrigin::ProcessSubstitution)
                .count()
                == 2
        );
    }

    #[test]
    fn heredoc_govdesi_yutulur() {
        let plan = parse("cat <<EOF\nrm -rf /\nEOF\ngit status");
        assert_eq!(binaries(&plan), ["cat", "git"]);
        assert_eq!(plan.heredocs.len(), 1);
        assert_eq!(plan.heredocs[0].body, "rm -rf /\n");
    }

    #[test]
    fn tirnaksiz_heredoc_govdesindeki_ikame_cikarilir() {
        let plan = parse("cargo test <<EOF\n$(cat /etc/shadow)\nEOF");
        assert!(binaries(&plan).contains(&"cat".to_string()));
        assert_eq!(
            plan.commands
                .iter()
                .find(|c| c.binary == "cat")
                .map(|c| c.origin),
            Some(CommandOrigin::HeredocExpansion)
        );
    }

    #[test]
    fn tirnakli_heredoc_govdesi_genislemez() {
        let plan = parse("cargo test <<'EOF'\n$(cat /etc/shadow)\nEOF");
        assert_eq!(binaries(&plan), ["cargo"]);
    }

    #[test]
    fn yonlendirme_ayristirilir() {
        let plan = parse("cargo test 2>&1 > /tmp/out");
        assert_eq!(binaries(&plan), ["cargo"]);
        assert_eq!(plan.redirections.len(), 2);
        assert_eq!(plan.redirections[0].op, "2>&1");
        assert_eq!(plan.redirections[1].op, ">");
        assert_eq!(plan.redirections[1].target, "/tmp/out");
    }

    #[test]
    fn tirnak_hilesi_cozulur() {
        assert_eq!(binaries(&parse("c\"a\"t /etc/passwd")), ["cat"]);
        assert_eq!(binaries(&parse("\\cat /etc/passwd")), ["cat"]);
        assert_eq!(binaries(&parse("'cat' x")), ["cat"]);
        assert_eq!(binaries(&parse("c''at x")), ["cat"]);
    }

    #[test]
    fn ortam_oneki_ikiliyi_gizlemez() {
        let plan = parse("PATH=/tmp cargo test");
        assert_eq!(binaries(&plan), ["cargo"]);
        assert_eq!(plan.commands[0].assignments, ["PATH=/tmp".to_string()]);
    }

    #[test]
    fn yorum_satiri_komut_gizlemez() {
        let plan = parse("cargo test # cat /etc/passwd");
        assert_eq!(binaries(&plan), ["cargo"]);
    }

    #[test]
    fn dinamik_ikili_isaretlenir() {
        let plan = parse("$CMD --help");
        assert_eq!(plan.commands.len(), 1);
        assert!(plan.commands[0].dynamic_binary);
    }

    #[test]
    fn kontrol_yapilari_kaydedilir() {
        let plan = parse("for i in 1 2; do cat x; done");
        assert!(plan.control_words.contains(&"for".to_string()));
        assert!(binaries(&plan).contains(&"cat".to_string()));
    }

    #[test]
    fn fonksiyon_tanimi_kaydedilir() {
        let plan = parse("evil() { cat /etc/passwd; }");
        assert_eq!(plan.function_definitions, ["evil".to_string()]);
        assert!(binaries(&plan).contains(&"cat".to_string()));
    }

    #[test]
    fn kapanmamis_tirnak_hata_dondurur() {
        assert_eq!(
            parse_shell("cargo test 'x"),
            Err(ShellParseError::UnterminatedQuote)
        );
    }

    #[test]
    fn kapanmamis_ikame_hata_dondurur() {
        assert_eq!(
            parse_shell("echo $(cat x"),
            Err(ShellParseError::UnterminatedSubstitution)
        );
    }

    #[test]
    fn izinli_komutlar_gecer() {
        let policy = ShellPolicy::new();
        for command in [
            "cargo test",
            "cargo build --release",
            "git status",
            "npm ci",
            "pytest -q tests",
        ] {
            assert!(
                policy.evaluate(command).is_allowed(),
                "{command}: {:?}",
                policy.evaluate(command).reason()
            );
        }
    }

    #[test]
    fn izin_disi_ikili_reddedilir() {
        let policy = ShellPolicy::new();
        let verdict = policy.evaluate("rustc --version");
        assert!(!verdict.is_allowed());
        assert_eq!(verdict.denials()[0].rule, DenyRule::NotAllowlisted);
    }

    #[test]
    fn kalici_red_listesi_izne_baskin() {
        let policy = ShellPolicy::new().allow_binary("bash");
        let verdict = policy.evaluate("bash -c 'id'");
        assert!(!verdict.is_allowed());
        assert_eq!(verdict.denials()[0].rule, DenyRule::NeverAllowed);
        assert!(!policy.allowed_binaries().contains(&"bash"));
    }

    #[test]
    fn yol_ile_cagri_reddedilir() {
        let verdict = ShellPolicy::new().evaluate("/usr/bin/cargo test");
        assert_eq!(verdict.denials()[0].rule, DenyRule::PathBinary);
    }

    #[test]
    fn git_pager_kacisi_reddedilir() {
        let verdict = ShellPolicy::new().evaluate("git -c core.pager=/tmp/x log");
        assert!(!verdict.is_allowed());
        assert!(verdict.denials().iter().any(|d| d.rule == DenyRule::ArgumentRule));
    }

    #[test]
    fn npm_run_reddedilir() {
        let verdict = ShellPolicy::new().evaluate("npm run rastgele-script");
        assert!(!verdict.is_allowed());
        assert_eq!(verdict.denials()[0].rule, DenyRule::ArgumentRule);
    }

    #[test]
    fn cargo_config_kacisi_reddedilir() {
        let verdict = ShellPolicy::new().evaluate("cargo --config target.x.runner=\"sh\" test");
        assert!(!verdict.is_allowed());
    }

    #[test]
    fn bos_komut_reddedilir() {
        assert!(!ShellPolicy::new().evaluate("   ").is_allowed());
    }

    #[test]
    fn ayristirma_hatasi_reddedilir() {
        let verdict = ShellPolicy::new().evaluate("cargo test $(");
        assert_eq!(verdict.denials()[0].rule, DenyRule::ParseError);
    }

    #[test]
    fn gevsetilmis_politika_yonlendirmeye_izin_verir() {
        let policy = ShellPolicy {
            allow_redirection: true,
            ..ShellPolicy::new()
        };
        assert!(policy.evaluate("cargo test > out.txt").is_allowed());
    }

    #[test]
    fn red_denetime_iki_satir_yazar() {
        let guard = ShellGuard::new(ShellPolicy::new());
        let (sink, mut stream) = audit_channel();
        let gate = AuditGate::new(sink);

        let result = guard.authorize_at(&gate, 5, "cat /etc/passwd", t0());
        assert!(matches!(result, Err(ToolsError::Denied(_))));

        match stream.try_recv() {
            Ok(AuditEvent::Capability(decision)) => {
                assert_eq!(decision.capability, CAPABILITY_SHELL);
                assert_eq!(decision.decision.label(), "deny");
            }
            other => panic!("yetki kaydi bekleniyordu: {other:?}"),
        }
        match stream.try_recv() {
            Ok(AuditEvent::ToolCall(record)) => {
                assert_eq!(record.tool, SHELL_TOOL);
                assert!(!record.capability_ok);
            }
            other => panic!("cagri kaydi bekleniyordu: {other:?}"),
        }
    }

    #[test]
    fn izin_tek_yetki_satiri_yazar() {
        let guard = ShellGuard::new(ShellPolicy::new());
        let (sink, mut stream) = audit_channel();
        let gate = AuditGate::new(sink);
        assert!(guard.authorize_at(&gate, 5, "cargo test", t0()).is_ok());
        match stream.try_recv() {
            Ok(AuditEvent::Capability(decision)) => assert!(decision.is_allowed()),
            other => panic!("yetki kaydi bekleniyordu: {other:?}"),
        }
        assert!(stream.try_recv().is_err());
    }

    #[test]
    fn gerekcesiz_acil_kacis_reddedilir() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        let request = EscapeRequest::at(9, "kisa", "cat x", 60, t0());
        assert!(guard.request_escape(&gate, request).is_err());
    }

    #[test]
    fn onaysiz_talep_izin_vermez() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        let request = EscapeRequest::at(
            9,
            "native hashline_read bu ikili dosyada calismiyor",
            "cat build.bin",
            60,
            t0(),
        );
        assert!(guard.request_escape(&gate, request).is_ok());
        assert!(guard.authorize_at(&gate, 9, "cat build.bin", t0()).is_err());
    }

    #[test]
    fn yargic_onayi_sureli_izin_verir() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        let now = t0();
        let request = EscapeRequest::at(
            9,
            "native hashline_read bu ikili dosyada calismiyor",
            "cat build.bin",
            120,
            now,
        );
        assert!(guard.request_escape(&gate, request).is_ok());

        let approved = guard.approve_escape_at(&gate, 9, "judge", 60, now);
        let grant = match approved {
            Ok((grant, record)) => {
                assert_eq!(record.source, INTERRUPT_SOURCE_JUDGE);
                assert!(record.resolved_at.is_some());
                grant
            }
            Err(err) => panic!("onay basarisiz: {err}"),
        };
        assert_eq!(grant.approver, "judge");

        // Sure icinde gecer.
        assert!(
            guard
                .authorize_at(&gate, 9, "cat build.bin", now + Duration::seconds(30))
                .is_ok()
        );
        // Sure dolunca politika yeniden zorlanir.
        assert!(
            guard
                .authorize_at(&gate, 9, "cat build.bin", now + Duration::seconds(61))
                .is_err()
        );
        // Baska ajan izinden yararlanamaz.
        assert!(guard.authorize_at(&gate, 10, "cat build.bin", now).is_err());
    }

    #[test]
    fn izin_geri_alinabilir() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        let now = t0();
        let request = EscapeRequest::at(
            3,
            "native tool ikili dosyada calismiyor, gecici shell gerekli",
            "cat x",
            120,
            now,
        );
        assert!(guard.request_escape(&gate, request).is_ok());
        assert!(guard.approve_escape_at(&gate, 3, "judge", 60, now).is_ok());
        assert!(guard.authorize_at(&gate, 3, "cat x", now).is_ok());
        assert!(guard.revoke_escape(3));
        assert!(guard.authorize_at(&gate, 3, "cat x", now).is_err());
    }

    #[test]
    fn onay_bekleyen_talep_yoksa_onay_reddedilir() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        assert!(
            guard
                .approve_escape_at(&gate, 42, "judge", 60, t0())
                .is_err()
        );
    }

    #[test]
    fn izin_suresi_ust_sinira_kirpilir() {
        let mut ledger = EscapeLedger::new().with_max_ttl_secs(30);
        let now = t0();
        let request = EscapeRequest::at(
            1,
            "native tool calismadi, gecici shell gerekli",
            "cat x",
            3600,
            now,
        );
        assert!(ledger.open(request).is_ok());
        match ledger.approve(1, "judge", 3600, now) {
            Ok((grant, _)) => {
                assert_eq!(grant.expires_at, now + Duration::seconds(30));
            }
            Err(err) => panic!("onay basarisiz: {err}"),
        }
    }

    #[test]
    fn yargic_reddi_izin_dogurmaz() {
        let mut guard = ShellGuard::new(ShellPolicy::new());
        let (sink, _stream) = audit_channel();
        let gate = AuditGate::new(sink);
        let now = t0();
        let request = EscapeRequest::at(
            4,
            "native tool calismadi, gecici shell gerekli",
            "cat x",
            60,
            now,
        );
        assert!(guard.request_escape(&gate, request).is_ok());
        match guard.reject_escape_at(&gate, 4, "judge", now) {
            Ok(record) => assert!(record.resolved_at.is_some()),
            Err(err) => panic!("red basarisiz: {err}"),
        }
        assert!(guard.authorize_at(&gate, 4, "cat x", now).is_err());
    }

    #[test]
    fn suresi_dolan_izin_temizlenir() {
        let mut ledger = EscapeLedger::new();
        let now = t0();
        let request = EscapeRequest::at(
            7,
            "native tool calismadi, gecici shell gerekli",
            "cat x",
            60,
            now,
        );
        assert!(ledger.open(request).is_ok());
        assert!(ledger.approve(7, "judge", 60, now).is_ok());
        assert!(ledger.active_grant(7, now).is_some());
        ledger.purge_expired(now + Duration::seconds(120));
        assert!(ledger.active_grant(7, now).is_none());
    }
}
