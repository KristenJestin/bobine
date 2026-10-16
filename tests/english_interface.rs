//! The interface is written in English (#39). This reads the program's own source and fails on a
//! string literal that still holds French: an accented letter, a French quotation mark, or a word
//! only French uses. What the server sends is data, and the invented library of the fake server
//! and the Jellyfin fixtures stand for it, so they are not read. Comments and the `mod tests` at
//! the end of each file are not read either (tests may hold French data to prove Unicode).

use std::fs;
use std::path::{Path, PathBuf};

/// French words that have no accent to give them away. Whole words, compared in lowercase.
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
    "lecture",
    "les",
    "lire",
    "liste",
    "mot",
    "ouvrez",
    "passe",
    "pour",
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
    "tout",
    "toutes",
    "tri",
    "une",
    "utilisateur",
    "voir",
    "votre",
    "vu",
    "vus",
];

/// The files that stand for the server's data, not for the program's words.
const SERVER_DATA: &[&str] = &["src/fake.rs", "src/jellyfin"];

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

/// The code before `#[cfg(test)] mod …`, the end of the file when there is none.
fn before_tests(source: &str) -> &str {
    let mut offset = 0;
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines.peek().is_some_and(|next| {
                let next = next.trim_start_matches("pub ").trim_start();
                next.starts_with("mod ")
            })
        {
            return &source[..offset];
        }
        offset += line.len() + 1;
    }
    source
}

/// The string literals of some Rust code with their line number, leaving out comments and char
/// literals.
fn literals(code: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = code.chars().collect();
    let (mut found, mut line, mut i) = (Vec::new(), 1, 0);
    while i < chars.len() {
        match chars[i] {
            '\n' => line += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    line += usize::from(chars[i] == '\n');
                    i += 1;
                }
                i += 2;
                continue;
            }
            // 'x', '\n', '"': a char literal. Anything else after a quote is a lifetime.
            '\'' => {
                if chars.get(i + 1) == Some(&'\\') {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                } else if chars.get(i + 2) == Some(&'\'') {
                    i += 2;
                }
            }
            '"' => {
                let (start, text) = (line, i + 1);
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    line += usize::from(chars.get(i) == Some(&'\n'));
                    i += 1;
                }
                found.push((start, chars[text..i.min(chars.len())].iter().collect()));
            }
            _ => {}
        }
        i += 1;
    }
    found
}

fn french_in(literal: &str) -> Option<String> {
    if let Some(letter) = literal.chars().find(|c| {
        ('\u{c0}'..='\u{ff}').contains(c) && !matches!(c, '\u{d7}' | '\u{f7}')
            || "«»œŒ".contains(*c)
    }) {
        return Some(format!("the letter {letter}"));
    }
    literal
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .find(|word| FRENCH_WORDS.contains(&word.as_str()))
        .map(|word| format!("the word {word}"))
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
        if SERVER_DATA.iter().any(|data| name.starts_with(data)) {
            continue;
        }
        let source = fs::read_to_string(&file).unwrap();
        for (line, literal) in literals(before_tests(&source)) {
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
