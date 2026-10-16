//! The interface is written in English (#39). This reads the program's own source and fails on a
//! string literal that still holds French: an accented letter, a French quotation mark, a
//! no-break space, or a word only French uses. What the server sends is data, and the invented
//! library of the fake server and the Jellyfin fixtures stand for it, so they are not read.
//! Comments and the body of a `#[cfg(test)] mod … { … }` are not read either (tests may hold
//! French data to prove Unicode).

use std::fs;
use std::path::{Path, PathBuf};

/// French words that have no accent to give them away, and that English does not use. Whole
/// words, compared in lowercase. A word English also spells (`pour`, `lecture`, `tout`, `les`)
/// is not here: it would flag English, so French is caught by the phrases below instead.
const FRENCH_WORDS: &[&str] = &[
    "accueil",
    "adresse",
    "annuler",
    "aucun",
    "aucune",
    "avec",
    "chargement",
    "cette",
    "connecter",
    "connexion",
    "dans",
    "des",
    "directe",
    "fermer",
    "fichier",
    "fiche",
    "ouvrir",
    "lecteur",
    "liste",
    "mot",
    "ouvrez",
    "passe",
    "proches",
    "recherche",
    "rechercher",
    "reprendre",
    "reste",
    "rien",
    "saison",
    "saisons",
    "serveur",
    "titre",
    "titres",
    "tous",
    "toutes",
    "une",
    "utilisateur",
    "voir",
    "votre",
    "vus",
];

/// Runs of words that are French, though each word alone could be English. Whole words in a
/// row, compared in lowercase.
const FRENCH_PHRASES: &[&[&str]] = &[
    &["lire", "la"],
    &["les", "films"],
    &["les", "titres"],
    &["ma", "liste"],
    &["mot", "de", "passe"],
    &["pour", "commencer"],
    &["tout", "lire"],
    &["tri", "par"],
    &["trier", "par"],
];

/// The files that stand for the server's data, not for the program's words.
const SERVER_DATA: &[&str] = &[
    "src/fake.rs",
    "src/jellyfin/fake.rs",
    "src/jellyfin/tests.rs",
];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    /// The text between the quotes of a string, raw or not, as written (escapes are not undone).
    Str(String),
    Word(String),
    Sym(char),
}

/// The tokens of some Rust code with their line number. Comments (nested block comments too)
/// and char literals are left out; strings are whole tokens, so a brace or a quote inside one
/// never reads as code.
fn tokens(code: &str) -> Vec<(usize, Token)> {
    let chars: Vec<char> = code.chars().collect();
    let (mut found, mut line, mut i) = (Vec::new(), 1, 0);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => line += 1,
            _ if c.is_whitespace() => {}
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let mut depth = 0;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        line += usize::from(chars[i] == '\n');
                        i += 1;
                    }
                }
                continue;
            }
            '"' => {
                let (text, end) = quoted(&chars, i + 1);
                found.push((line, Token::Str(text)));
                line += lines_in(&chars[i..end]);
                i = end;
                continue;
            }
            '\'' => {
                // 'x', '\n', '\'', '"': a char literal. Anything else after a quote is a lifetime.
                if chars.get(i + 1) == Some(&'\\') {
                    i += 3;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                } else if chars.get(i + 2) == Some(&'\'') {
                    i += 2;
                }
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let next = chars.get(i).copied();
                match (word.as_str(), next) {
                    // b"…", c"…": a string with a prefix.
                    ("b" | "c", Some('"')) => {
                        let (text, end) = quoted(&chars, i + 1);
                        found.push((line, Token::Str(text)));
                        line += lines_in(&chars[i..end]);
                        i = end;
                    }
                    // b'x': a byte literal.
                    ("b", Some('\'')) => {
                        i += if chars.get(i + 1) == Some(&'\\') {
                            3
                        } else {
                            2
                        };
                        while i < chars.len() && chars[i] != '\'' {
                            i += 1;
                        }
                        i += 1;
                    }
                    // r"…", r#"…"#, br"…", cr#"…"#: a raw string. `r#type` is a name.
                    ("r" | "br" | "cr", Some('"' | '#')) => {
                        let hashes = chars[i..].iter().take_while(|&&c| c == '#').count();
                        if chars.get(i + hashes) == Some(&'"') {
                            let (text, end) = raw(&chars, i + hashes + 1, hashes);
                            found.push((line, Token::Str(text)));
                            line += lines_in(&chars[i..end]);
                            i = end;
                        } else {
                            found.push((line, Token::Word(word)));
                        }
                    }
                    _ => found.push((line, Token::Word(word))),
                }
                continue;
            }
            _ => found.push((line, Token::Sym(c))),
        }
        i += 1;
    }
    found
}

fn lines_in(chars: &[char]) -> usize {
    chars.iter().filter(|&&c| c == '\n').count()
}

/// The text of a string whose opening quote is just before `from`, and where it ends.
fn quoted(chars: &[char], from: usize) -> (String, usize) {
    let mut i = from;
    while i < chars.len() && chars[i] != '"' {
        i += if chars[i] == '\\' { 2 } else { 1 };
    }
    let end = i.min(chars.len());
    (chars[from..end].iter().collect(), end + 1)
}

/// The text of a raw string whose opening quote is just before `from`, closed by a quote and
/// `hashes` hash marks, and where it ends.
fn raw(chars: &[char], from: usize, hashes: usize) -> (String, usize) {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '"' && chars[i + 1..].iter().take_while(|&&c| c == '#').count() >= hashes {
            return (chars[from..i].iter().collect(), i + 1 + hashes);
        }
        i += 1;
    }
    (chars[from..].iter().collect(), chars.len())
}

/// The string literals of some Rust code with their line number, leaving out comments, char
/// literals and the body of every `#[cfg(test)] mod name { … }`. A `#[cfg(test)] mod name;`
/// has no body here: the file it names is read like any other.
fn literals(code: &str) -> Vec<(usize, String)> {
    const TEST_MOD: [&str; 7] = ["#", "[", "cfg", "(", "test", ")", "]"];
    let tokens = tokens(code);
    let text = |token: &Token| match token {
        Token::Word(word) => word.clone(),
        Token::Sym(sym) => sym.to_string(),
        Token::Str(_) => String::new(),
    };
    let (mut found, mut i) = (Vec::new(), 0);
    while i < tokens.len() {
        if i + TEST_MOD.len() <= tokens.len()
            && TEST_MOD
                .iter()
                .zip(&tokens[i..])
                .all(|(want, (_, token))| text(token) == *want)
        {
            // `pub`, `pub(crate)`, then `mod name`, then `{` or `;`.
            let mut at = i + TEST_MOD.len();
            if tokens.get(at).map(|(_, t)| text(t)).as_deref() == Some("pub") {
                at += 1;
                if tokens.get(at).map(|(_, t)| text(t)).as_deref() == Some("(") {
                    while tokens.get(at).is_some_and(|(_, t)| text(t) != ")") {
                        at += 1;
                    }
                    at += 1;
                }
            }
            if tokens.get(at).map(|(_, t)| text(t)).as_deref() == Some("mod")
                && tokens.get(at + 2).map(|(_, t)| text(t)).as_deref() == Some("{")
            {
                let mut depth = 0;
                at += 2;
                while let Some((_, token)) = tokens.get(at) {
                    match token {
                        Token::Sym('{') => depth += 1,
                        Token::Sym('}') => depth -= 1,
                        _ => {}
                    }
                    at += 1;
                    if depth == 0 {
                        break;
                    }
                }
                i = at;
                continue;
            }
        }
        if let (line, Token::Str(string)) = &tokens[i] {
            found.push((*line, string.clone()));
        }
        i += 1;
    }
    found
}

/// The text with its `\u{…}` escapes undone, so a French letter or a no-break space written as
/// an escape is seen like one written out.
fn unescaped(literal: &str) -> String {
    let mut out = String::new();
    let mut rest = literal;
    while let Some(at) = rest.find("\\u{") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 3..];
        match after.find('}') {
            Some(end) => {
                match u32::from_str_radix(&after[..end], 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    Some(c) => out.push(c),
                    None => out.push_str(&rest[at..at + 3 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn french_in(literal: &str) -> Option<String> {
    let literal = unescaped(literal);
    if let Some(space) = literal.chars().find(|c| matches!(c, '\u{a0}' | '\u{202f}')) {
        return Some(format!("the no-break space U+{:04X}", space as u32));
    }
    if let Some(letter) = literal.chars().find(|c| {
        ('\u{c0}'..='\u{ff}').contains(c) && !matches!(c, '\u{d7}' | '\u{f7}')
            || "«»œŒ".contains(*c)
    }) {
        return Some(format!("the letter {letter}"));
    }
    let words: Vec<String> = literal
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    if let Some(word) = words
        .iter()
        .find(|word| FRENCH_WORDS.contains(&word.as_str()))
    {
        return Some(format!("the word {word}"));
    }
    FRENCH_PHRASES
        .iter()
        .find(|phrase| {
            words
                .windows(phrase.len())
                .any(|run| run.iter().zip(**phrase).all(|(word, want)| word == want))
        })
        .map(|phrase| format!("the words {}", phrase.join(" ")))
}

#[test]
fn no_string_of_the_program_is_in_french() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut french = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if SERVER_DATA.contains(&name.as_str()) {
            continue;
        }
        let source = fs::read_to_string(&file).unwrap();
        for (line, literal) in literals(&source) {
            if let Some(why) = french_in(&literal) {
                french.push(format!("{name}:{line}: {why} in {literal:?}"));
            }
        }
    }
    assert!(
        french.is_empty(),
        "French left in the interface:\n{}",
        french.join("\n")
    );
}

/// What the interface no longer says: the list is "Favorites" (#60), in US English. A name
/// joined by `-` or `_` (`my-list`, `--favourite-fails`) is an identifier, not a word shown.
fn old_wording_in(literal: &str) -> Option<&'static str> {
    let words: Vec<String> = literal
        .split(|c: char| !(c.is_alphabetic() || c == '-' || c == '_'))
        .filter(|word| !word.is_empty() && !word.contains(['-', '_']))
        .map(str::to_lowercase)
        .collect();
    if words.windows(2).any(|run| run == ["my", "list"]) {
        return Some("My list");
    }
    words
        .iter()
        .any(|word| word == "favourite" || word == "favourites")
        .then_some("the UK Favourite")
}

#[test]
fn no_string_of_the_program_says_my_list_or_the_uk_favourite() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    let mut old = Vec::new();
    for file in files {
        let name = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if SERVER_DATA.contains(&name.as_str()) {
            continue;
        }
        let source = fs::read_to_string(&file).unwrap();
        for (line, literal) in literals(&source) {
            if let Some(why) = old_wording_in(&literal) {
                old.push(format!("{name}:{line}: {why} in {literal:?}"));
            }
        }
    }
    assert!(
        old.is_empty(),
        "Old wording left in the interface:\n{}",
        old.join("\n")
    );
}

#[test]
fn the_old_wording_is_seen_whatever_its_case() {
    assert_eq!(old_wording_in("Add to My list"), Some("My list"));
    assert_eq!(old_wording_in("MY LIST"), Some("My list"));
    assert_eq!(old_wording_in("Favourite"), Some("the UK Favourite"));
    assert_eq!(
        old_wording_in("No favourites yet."),
        Some("the UK Favourite")
    );
    assert_eq!(old_wording_in("Favorite"), None);
    assert_eq!(old_wording_in("my listing"), None);
    assert_eq!(old_wording_in("my-list"), None);
    assert_eq!(old_wording_in("--favourite-fails"), None);
}

#[test]
fn the_reader_of_literals_skips_comments_chars_and_lifetimes() {
    let code = "// été\nlet a = 'é'; let q = '\"'; fn f<'a>(x: &'a str) {} /* Lecture */\n\
                let s = \"Rien\\n\"; let t = \"deux\nlignes\"; let u = \"fin\";";
    assert_eq!(
        literals(code),
        [
            (3, "Rien\\n".to_string()),
            (3, "deux\nlignes".to_string()),
            (4, "fin".to_string()),
        ]
    );
}

#[test]
fn an_escaped_quote_in_a_char_or_a_string_does_not_open_a_string() {
    let code = "let a = '\\''; let b = \"it\\\"s\"; let c = '\\u{e9}'; let d = \"x\";";
    assert_eq!(
        literals(code),
        [(1, "it\\\"s".to_string()), (1, "x".to_string())]
    );
}

#[test]
fn nested_block_comments_hide_their_strings() {
    assert_eq!(
        literals("/* a /* \"hidden\" */ \"still hidden\" */ let a = \"seen\";"),
        [(1, "seen".to_string())]
    );
}

#[test]
fn the_reader_of_literals_reads_raw_and_byte_strings() {
    let code = "let a = r\"un \\ « \"; let b = r#\"dit \"oui\" ici\"#; let c = br##\"x\"# y\"##;\n\
                let d = b\"octets\"; let e = b'\"'; let r#type = \"after\"; let f = c\"cstr\";";
    assert_eq!(
        literals(code),
        [
            (1, "un \\ « ".to_string()),
            (1, "dit \"oui\" ici".to_string()),
            (1, "x\"# y".to_string()),
            (2, "octets".to_string()),
            (2, "after".to_string()),
            (2, "cstr".to_string()),
        ]
    );
}

#[test]
fn only_the_body_of_a_test_module_is_left_out() {
    let code = "const A: &str = \"before\";\n\
                #[cfg(test)]\nmod helper;\n\
                const B: &str = \"after the declaration\";\n\
                #[cfg(test)]\npub mod fake {\n    fn f() { let s = \"} {\"; if true { let _ = \"inner\"; } }\n}\n\
                const C: &str = \"after the module\";\n\
                #[cfg(test)]\nmod tests {\n    const D: &str = \"in tests\";\n}\n\
                const E: &str = \"at the end\";";
    let seen: Vec<String> = literals(code).into_iter().map(|(_, s)| s).collect();
    assert_eq!(
        seen,
        [
            "before",
            "after the declaration",
            "after the module",
            "at the end",
        ]
    );
}

#[test]
fn french_is_told_from_english() {
    assert_eq!(french_in("Black out other screens"), None);
    assert_eq!(french_in("1h 28m left"), None);
    assert_eq!(french_in("Résultat").as_deref(), Some("the letter é"));
    assert_eq!(
        french_in("Chargement…").as_deref(),
        Some("the word chargement")
    );
    assert_eq!(french_in("Aucun film").as_deref(), Some("the word aucun"));
    assert_eq!(french_in("“Quoted”"), None);
    assert_eq!(french_in("« cité »").as_deref(), Some("the letter «"));
}

#[test]
fn english_words_that_are_also_french_are_not_flagged() {
    for english in ["Pour", "Lecture notes", "Tout", "Les", "Vu", "Sort by"] {
        assert_eq!(french_in(english), None, "{english}");
    }
    assert_eq!(
        french_in("Lire la suite").as_deref(),
        Some("the words lire la")
    );
    assert_eq!(
        french_in("Jellyfin pour commencer").as_deref(),
        Some("the words pour commencer")
    );
}

#[test]
fn a_no_break_space_is_flagged_written_out_or_escaped() {
    assert_eq!(
        french_in("1\u{a0}284").as_deref(),
        Some("the no-break space U+00A0")
    );
    assert_eq!(
        french_in("1\u{202f}284").as_deref(),
        Some("the no-break space U+202F")
    );
    assert_eq!(
        french_in("1\\u{a0}284").as_deref(),
        Some("the no-break space U+00A0")
    );
    assert_eq!(
        french_in("1\\u{202F}284").as_deref(),
        Some("the no-break space U+202F")
    );
    assert_eq!(french_in("caf\\u{e9}").as_deref(), Some("the letter é"));
    assert_eq!(french_in("1,284"), None);
}
