//! The tools above a library grid: what narrows it (genre, decade, unwatched only), how it is
//! sorted, and the alphabet jump. They are asked of the server, over the whole library, never
//! over the pages already loaded; this file only says what to ask.

use bobine::provider::{FilmQuery, FilmSort, Filters};

use crate::grid::PAGE;

/// The alphabet bar: `#` for titles that do not start with a letter, then A to Z.
pub const LETTERS: [char; 27] = [
    '#', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

/// Where a letter stands on the bar, from the top.
pub fn letter_index(letter: char) -> Option<usize> {
    LETTERS.iter().position(|&l| l == letter)
}

/// How much the letter at `index` is the current one while the highlight is at `position` of
/// the bar: all of it on the spot, none a letter away, so that it moves from one to the other.
pub fn letter_weight(position: f32, index: usize) -> f32 {
    (1. - (position - index as f32).abs()).clamp(0., 1.)
}

/// How a grid is ordered. Each way has the direction that suits it: the newest, the best rated
/// first, the titles from A to Z.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Added,
    Title,
    Year,
    Rating,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Added, Sort::Title, Sort::Year, Sort::Rating];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Added => "Ajout récent",
            Sort::Title => "Titre",
            Sort::Year => "Année de sortie",
            Sort::Rating => "Note",
        }
    }

    /// What the server sorts by, and whether it goes down.
    fn order(self) -> (FilmSort, bool) {
        match self {
            Sort::Added => (FilmSort::DateAdded, true),
            Sort::Title => (FilmSort::Title, false),
            Sort::Year => (FilmSort::ReleaseYear, true),
            Sort::Rating => (FilmSort::Rating, true),
        }
    }
}

/// What a library grid shows now. Kept for as long as the program runs, one per library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    pub filters: Filters,
    pub sort: Sort,
}

impl Tools {
    /// The page of the grid, in the order and under the filters chosen.
    pub fn query(&self, page: u32) -> FilmQuery {
        let (sort, descending) = self.sort.order();
        FilmQuery {
            sort,
            descending,
            filters: self.filters.clone(),
            before: None,
            start: page * PAGE,
            limit: PAGE,
        }
    }

    /// The alphabet bar is for titles in alphabetical order, and only then.
    pub fn letters_shown(&self) -> bool {
        self.sort == Sort::Title
    }

    /// What to ask the server to find where `letter` starts: how many titles come before it,
    /// under the same filters. `None` for `#`, which starts the list.
    pub fn letter_query(&self, letter: char) -> Option<FilmQuery> {
        (letter != '#').then(|| FilmQuery {
            before: Some(letter),
            // One title is enough: the total the server answers with is the count.
            limit: 1,
            ..self.query(0)
        })
    }

    /// Whether anything narrows the grid.
    pub fn narrowed(&self) -> bool {
        self.filters != Filters::default()
    }
}

/// `Années 2010`.
pub fn decade_label(decade: i32) -> String {
    format!("Années {decade}")
}

/// The letter a title is listed under: its first letter without accent (`É` is `E`), or `#`.
pub fn letter_of(title: &str) -> char {
    let first = title.trim_start().chars().next().and_then(plain);
    first.filter(char::is_ascii_alphabetic).unwrap_or('#')
}

/// The capital a letter is listed under: `é` and `É` are `E`. Anything that is not a letter
/// of the Latin alphabet comes back as it is, in capitals when it has them.
fn plain(letter: char) -> Option<char> {
    let capital = letter.to_uppercase().next()?;
    Some(match capital {
        'À'..='Å' | 'Æ' | 'Ā'..='Ą' => 'A',
        'Ç' | 'Ć'..='Č' => 'C',
        'Ð' | 'Ď'..='Đ' => 'D',
        'È'..='Ë' | 'Ē'..='Ě' => 'E',
        'Ĝ'..='Ģ' => 'G',
        'Ĥ'..='Ħ' => 'H',
        'Ì'..='Ï' | 'Ĩ'..='İ' => 'I',
        'Ĵ' => 'J',
        'Ķ' => 'K',
        'Ĺ'..='Ł' => 'L',
        'Ñ' | 'Ń'..='Ŋ' => 'N',
        'Ò'..='Ö' | 'Ø' | 'Ō'..='Œ' => 'O',
        'Ŕ'..='Ř' => 'R',
        'Ś'..='Š' => 'S',
        'Ţ'..='Ŧ' => 'T',
        'Ù'..='Ü' | 'Ũ'..='Ų' => 'U',
        'Ŵ' => 'W',
        'Ý' | 'Ŷ'..='Ÿ' => 'Y',
        'Ź'..='Ž' => 'Z',
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_stands_where_the_bar_draws_it() {
        assert_eq!(letter_index('#'), Some(0));
        assert_eq!(letter_index('A'), Some(1));
        assert_eq!(letter_index('Z'), Some(26));
        assert_eq!(letter_index('é'), None);
    }

    #[test]
    fn the_highlight_is_wholly_on_a_letter_it_rests_on() {
        assert_eq!(letter_weight(6., 6), 1.);
        assert_eq!(letter_weight(6., 5), 0.);
        assert_eq!(letter_weight(6., 7), 0.);
        assert_eq!(letter_weight(6., 20), 0.);
    }

    #[test]
    fn the_highlight_between_two_letters_is_shared_by_them() {
        assert_eq!(letter_weight(6.25, 6), 0.75);
        assert_eq!(letter_weight(6.25, 7), 0.25);
        assert_eq!(letter_weight(6.25, 5), 0.);
    }

    #[test]
    fn a_new_grid_is_the_whole_library_newest_first() {
        let tools = Tools::default();
        let q = tools.query(2);
        assert_eq!((q.sort, q.descending), (FilmSort::DateAdded, true));
        assert_eq!((q.start, q.limit), (2 * PAGE, PAGE));
        assert_eq!(q.filters, Filters::default());
        assert_eq!(q.before, None);
        assert!(!tools.narrowed());
    }

    #[test]
    fn each_sort_asks_the_server_for_its_own_order() {
        let ask = |sort| {
            let q = Tools {
                sort,
                ..Tools::default()
            }
            .query(0);
            (q.sort, q.descending)
        };
        assert_eq!(ask(Sort::Added), (FilmSort::DateAdded, true));
        assert_eq!(ask(Sort::Title), (FilmSort::Title, false));
        assert_eq!(ask(Sort::Year), (FilmSort::ReleaseYear, true));
        assert_eq!(ask(Sort::Rating), (FilmSort::Rating, true));
    }

    #[test]
    fn the_sorts_are_named_in_french() {
        let names: Vec<_> = Sort::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(names, ["Ajout récent", "Titre", "Année de sortie", "Note"]);
    }

    #[test]
    fn the_filters_go_to_the_query_of_every_page() {
        let tools = Tools {
            filters: Filters {
                genre: Some("Drame".into()),
                decade: Some(2010),
                unwatched: true,
            },
            sort: Sort::Rating,
        };
        for page in [0, 5] {
            let q = tools.query(page);
            assert_eq!(q.filters, tools.filters);
        }
        assert!(tools.narrowed());
    }

    #[test]
    fn any_one_filter_narrows_the_grid() {
        for filters in [
            Filters {
                genre: Some("Drame".into()),
                ..Filters::default()
            },
            Filters {
                decade: Some(1990),
                ..Filters::default()
            },
            Filters {
                unwatched: true,
                ..Filters::default()
            },
        ] {
            assert!(
                Tools {
                    filters,
                    ..Tools::default()
                }
                .narrowed()
            );
        }
    }

    #[test]
    fn the_alphabet_bar_shows_only_when_sorted_by_title() {
        for sort in Sort::ALL {
            let tools = Tools {
                sort,
                ..Tools::default()
            };
            assert_eq!(tools.letters_shown(), sort == Sort::Title, "{sort:?}");
        }
    }

    #[test]
    fn the_bar_goes_from_hash_to_z() {
        assert_eq!(LETTERS[0], '#');
        assert_eq!(LETTERS[1], 'A');
        assert_eq!(LETTERS[26], 'Z');
        assert_eq!(LETTERS.len(), 27);
    }

    #[test]
    fn a_letter_is_found_by_counting_the_titles_before_it_under_the_same_filters() {
        let tools = Tools {
            filters: Filters {
                unwatched: true,
                ..Filters::default()
            },
            sort: Sort::Title,
        };
        let q = tools.letter_query('D').unwrap();
        assert_eq!(q.before, Some('D'));
        assert_eq!(q.filters, tools.filters);
        assert_eq!((q.sort, q.descending), (FilmSort::Title, false));
        // One title is enough: the server's total is the count.
        assert_eq!((q.start, q.limit), (0, 1));
    }

    #[test]
    fn hash_starts_the_list_and_needs_no_question() {
        let tools = Tools {
            sort: Sort::Title,
            ..Tools::default()
        };
        assert_eq!(tools.letter_query('#'), None);
    }

    #[test]
    fn a_decade_reads_in_french() {
        assert_eq!(decade_label(2010), "Années 2010");
    }

    #[test]
    fn a_title_is_listed_under_its_first_letter() {
        assert_eq!(letter_of("Amber Fields"), 'A');
        assert_eq!(letter_of("lanterns"), 'L');
    }

    #[test]
    fn an_accented_letter_is_listed_with_its_plain_one() {
        assert_eq!(letter_of("Éclipse"), 'E');
        assert_eq!(letter_of("élan"), 'E');
        assert_eq!(letter_of("Ça va"), 'C');
        assert_eq!(letter_of("Œuvre"), 'O');
        assert_eq!(letter_of("Ångström"), 'A');
        assert_eq!(letter_of("Über"), 'U');
        assert_eq!(letter_of("Ñandú"), 'N');
    }

    #[test]
    fn a_title_that_does_not_start_with_a_letter_is_listed_under_hash() {
        assert_eq!(letter_of("1984 Remix"), '#');
        assert_eq!(letter_of("(Untitled)"), '#');
        assert_eq!(letter_of("¿Quién?"), '#');
        assert_eq!(letter_of(""), '#');
        assert_eq!(letter_of("  "), '#');
    }
}
