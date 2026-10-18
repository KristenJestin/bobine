//! The Stats page's numbers, from the watch history: no window here. Periods are on the local
//! clock, and a week starts on Monday.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use bobine::provider::{
    HistorySource, ItemId, LocalTime, Play, WatchHistory, Watched, WatchedKind,
};

/// How many titles of each kind, and how many genres, the page shows.
const TOP_TITLES: usize = 10;
const TOP_GENRES: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub source: HistorySource,
    /// Seconds watched this week, this month and this year.
    pub week: u64,
    pub month: u64,
    pub year: u64,
    pub movies: Vec<Ranked>,
    pub series: Vec<Ranked>,
    /// Seconds watched of each genre, a title counting for each of its genres.
    pub genres: Vec<(String, u64)>,
    /// Seconds watched each day of the week, Monday first, and each hour of the day: only a
    /// source that knows when each play happened has them.
    pub days: Option<[u64; 7]>,
    pub hours: Option<[u64; 24]>,
    pub review: Review,
}

/// A title, with how long and how many times it was watched.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    pub title: Watched,
    pub seconds: u64,
    pub plays: u32,
}

/// This year, in one card.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub year: i32,
    pub seconds: u64,
    pub top_title: Option<String>,
    pub top_genre: Option<String>,
    /// 1 to 12.
    pub busiest_month: Option<u32>,
}

/// None when nothing was watched: there is no page then.
pub fn of(history: &WatchHistory, now: LocalTime) -> Option<Stats> {
    if history.plays.is_empty() {
        return None;
    }
    let week_start = now.days() - now.weekday() as i64;
    let within = |play: &Play, keep: &dyn Fn(LocalTime) -> bool| -> u64 {
        play.at
            .filter(|at| at.days() <= now.days() && keep(*at))
            .map_or(0, |_| play.seconds)
    };
    let sum = |keep: &dyn Fn(LocalTime) -> bool| -> u64 {
        history.plays.iter().map(|play| within(play, keep)).sum()
    };
    let this_year = |at: LocalTime| at.year == now.year;
    let year_plays: Vec<&Play> = history
        .plays
        .iter()
        .filter(|play| within(play, &this_year) > 0)
        .collect();
    // A play dated after today (a clock set wrong) is counted nowhere.
    let all: Vec<&Play> = history
        .plays
        .iter()
        .filter(|play| play.at.is_none_or(|at| at.days() <= now.days()))
        .collect();
    let timed = history.source == HistorySource::PlaybackReporting;
    let mut days = [0; 7];
    let mut hours = [0; 24];
    for play in &all {
        if let Some(at) = play.at {
            days[at.weekday()] += play.seconds;
            hours[at.hour as usize] += play.seconds;
        }
    }
    let mut months = [0u64; 12];
    for play in &year_plays {
        if let Some(at) = play.at {
            months[at.month as usize - 1] += play.seconds;
        }
    }
    let busiest_month = (0..12)
        .filter(|month| months[*month] > 0)
        .max_by_key(|month| (months[*month], std::cmp::Reverse(*month)))
        .map(|month| month as u32 + 1);
    let year_titles = ranked(&year_plays);
    let all_titles = ranked(&all);
    let year_genres = genres(&year_plays);
    Some(Stats {
        source: history.source,
        week: sum(&|at| at.days() >= week_start),
        month: sum(&|at| at.year == now.year && at.month == now.month),
        year: sum(&this_year),
        movies: of_kind(&all_titles, WatchedKind::Movie, TOP_TITLES),
        series: of_kind(&all_titles, WatchedKind::Series, TOP_TITLES),
        genres: top(genres(&all), TOP_GENRES),
        days: timed.then_some(days),
        hours: timed.then_some(hours),
        review: Review {
            year: now.year,
            seconds: year_plays.iter().map(|play| play.seconds).sum(),
            top_title: year_titles.first().map(|first| first.title.name.clone()),
            top_genre: year_genres.first().map(|(genre, _)| genre.clone()),
            busiest_month,
        },
    })
}

fn top<T>(mut list: Vec<T>, count: usize) -> Vec<T> {
    list.truncate(count);
    list
}

/// The titles of `plays`, the most watched first. A title the library lost is known by its name.
fn ranked(plays: &[&Play]) -> Vec<Ranked> {
    let mut titles: Vec<Ranked> = Vec::new();
    let mut at: HashMap<(WatchedKind, Option<&ItemId>, &str), usize> = HashMap::new();
    for play in plays {
        let title = &play.title;
        let name = if title.id.is_some() { "" } else { &title.name };
        match at.entry((title.kind, title.id.as_ref(), name)) {
            Entry::Occupied(found) => {
                let ranked = &mut titles[*found.get()];
                ranked.seconds += play.seconds;
                ranked.plays += play.count;
            }
            Entry::Vacant(new) => {
                new.insert(titles.len());
                titles.push(Ranked {
                    title: title.clone(),
                    seconds: play.seconds,
                    plays: play.count,
                });
            }
        }
    }
    titles.sort_by(|a, b| {
        (b.seconds, b.plays)
            .cmp(&(a.seconds, a.plays))
            .then_with(|| a.title.name.cmp(&b.title.name))
    });
    titles
}

/// The first `count` of `titles` of `kind`.
fn of_kind(titles: &[Ranked], kind: WatchedKind, count: usize) -> Vec<Ranked> {
    titles
        .iter()
        .filter(|ranked| ranked.title.kind == kind)
        .take(count)
        .cloned()
        .collect()
}

/// Each genre of `plays` and the time watched of it, the most watched first.
fn genres(plays: &[&Play]) -> Vec<(String, u64)> {
    let mut seconds: HashMap<&str, u64> = HashMap::new();
    for play in plays {
        for genre in &play.title.genres {
            *seconds.entry(genre).or_default() += play.seconds;
        }
    }
    let mut genres: Vec<(String, u64)> = seconds
        .into_iter()
        .map(|(genre, seconds)| (genre.to_string(), seconds))
        .collect();
    genres.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    genres
}

/// `12 h 30 min`, `45 min`, `3 h`, to the nearest minute.
pub fn length(seconds: u64) -> String {
    let minutes = (seconds + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

pub fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES[(month as usize).clamp(1, 12) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saturday 17 October 2026, 21:00.
    fn now() -> LocalTime {
        at(2026, 10, 17, 21)
    }

    fn at(year: i32, month: u32, day: u32, hour: u32) -> LocalTime {
        LocalTime {
            year,
            month,
            day,
            hour,
            minute: 0,
        }
    }

    fn title(name: &str, kind: WatchedKind, genres: &[&str]) -> Watched {
        Watched {
            id: Some(ItemId(name.to_lowercase())),
            name: name.into(),
            kind,
            poster: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
        }
    }

    fn movie(name: &str, genres: &[&str]) -> Watched {
        title(name, WatchedKind::Movie, genres)
    }

    fn series(name: &str, genres: &[&str]) -> Watched {
        title(name, WatchedKind::Series, genres)
    }

    fn play(title: Watched, at: Option<LocalTime>, minutes: u64) -> Play {
        Play {
            title,
            at,
            seconds: minutes * 60,
            count: 1,
        }
    }

    fn history(source: HistorySource, plays: Vec<Play>) -> WatchHistory {
        WatchHistory { source, plays }
    }

    fn plugin(plays: Vec<Play>) -> Stats {
        of(&history(HistorySource::PlaybackReporting, plays), now()).unwrap()
    }

    #[test]
    fn the_week_starts_on_monday_and_periods_use_local_time() {
        let harbour = movie("The Quiet Harbour", &[]);
        let stats = plugin(vec![
            // Monday of this week, Sunday before it, earlier this month, this year, last year.
            play(harbour.clone(), Some(at(2026, 10, 12, 0)), 60),
            play(harbour.clone(), Some(at(2026, 10, 11, 23)), 30),
            play(harbour.clone(), Some(at(2026, 10, 2, 20)), 15),
            play(harbour.clone(), Some(at(2026, 3, 2, 20)), 120),
            play(harbour.clone(), Some(at(2025, 10, 16, 20)), 600),
            play(harbour, None, 1000),
        ]);
        assert_eq!(stats.week, 60 * 60);
        assert_eq!(stats.month, (60 + 30 + 15) * 60);
        assert_eq!(stats.year, (60 + 30 + 15 + 120) * 60);
    }

    #[test]
    fn hours_are_read_with_their_units() {
        assert_eq!(length(12 * 3600 + 30 * 60), "12 h 30 min");
        assert_eq!(length(45 * 60), "45 min");
        assert_eq!(length(3 * 3600 + 10), "3 h");
        assert_eq!(length(0), "0 min");
    }

    #[test]
    fn the_most_watched_titles_come_first_movies_and_series_apart() {
        let harbour = movie("The Quiet Harbour", &[]);
        let amber = movie("Amber Fields", &[]);
        let tidewater = series("Tidewater", &[]);
        let stats = plugin(vec![
            play(harbour.clone(), Some(now()), 60),
            play(amber.clone(), Some(now()), 100),
            play(harbour, Some(now()), 50),
            play(tidewater.clone(), Some(now()), 45),
            play(tidewater, Some(now()), 45),
        ]);
        let names = |list: &[Ranked]| -> Vec<(String, u64, u32)> {
            list.iter()
                .map(|ranked| (ranked.title.name.clone(), ranked.seconds / 60, ranked.plays))
                .collect()
        };
        assert_eq!(
            names(&stats.movies),
            [
                ("The Quiet Harbour".to_string(), 110, 2),
                ("Amber Fields".to_string(), 100, 1)
            ]
        );
        assert_eq!(names(&stats.series), [("Tidewater".to_string(), 90, 2)]);
    }

    #[test]
    fn a_title_the_library_lost_is_counted_by_its_name() {
        let gone = |name: &str| Watched {
            id: None,
            ..series(name, &[])
        };
        let stats = plugin(vec![
            play(gone("Lost Shore"), Some(now()), 30),
            play(gone("Lost Shore"), Some(now()), 30),
            play(gone("Far Isles"), Some(now()), 20),
        ]);
        assert_eq!(stats.series.len(), 2);
        assert_eq!(stats.series[0].plays, 2);
    }

    #[test]
    fn genres_are_ranked_by_time_watched() {
        let stats = plugin(vec![
            play(
                movie("The Quiet Harbour", &["Drama", "Mystery"]),
                Some(now()),
                100,
            ),
            play(series("Glass Orchard", &["Comedy"]), Some(now()), 150),
            play(movie("Amber Fields", &["Drama"]), Some(now()), 80),
        ]);
        assert_eq!(
            stats.genres,
            [
                ("Drama".to_string(), 180 * 60),
                ("Comedy".to_string(), 150 * 60),
                ("Mystery".to_string(), 100 * 60)
            ]
        );
    }

    #[test]
    fn with_the_plugin_the_activity_is_by_day_of_the_week_and_by_hour() {
        let harbour = movie("The Quiet Harbour", &[]);
        let stats = plugin(vec![
            play(harbour.clone(), Some(at(2026, 10, 12, 21)), 60),
            play(harbour.clone(), Some(at(2026, 10, 11, 8)), 30),
            play(harbour, Some(at(2026, 10, 5, 21)), 15),
        ]);
        let days = stats.days.expect("by day");
        assert_eq!(days[0], 75 * 60, "the Mondays");
        assert_eq!(days[6], 30 * 60, "the Sunday");
        let hours = stats.hours.expect("by hour");
        assert_eq!((hours[21], hours[8], hours[0]), (75 * 60, 30 * 60, 0));
    }

    #[test]
    fn without_the_plugin_there_is_no_activity_by_day_or_hour() {
        let history = history(
            HistorySource::PlayedItems,
            vec![play(movie("Amber Fields", &[]), Some(now()), 97)],
        );
        let stats = of(&history, now()).unwrap();
        assert_eq!(stats.source, HistorySource::PlayedItems);
        assert_eq!((stats.days, stats.hours), (None, None));
        assert_eq!(stats.movies.len(), 1);
    }

    #[test]
    fn with_no_data_at_all_there_are_no_stats() {
        for source in [HistorySource::PlaybackReporting, HistorySource::PlayedItems] {
            assert_eq!(of(&history(source, Vec::new()), now()), None);
        }
    }

    #[test]
    fn a_play_dated_after_today_is_counted_nowhere() {
        let harbour = movie("The Quiet Harbour", &["Drama"]);
        let ahead = movie("From Tomorrow", &["Western"]);
        let stats = plugin(vec![
            play(harbour, Some(at(2026, 10, 17, 21)), 60),
            play(ahead, Some(at(2026, 10, 18, 21)), 90),
        ]);
        let names: Vec<&str> = stats.movies.iter().map(|m| m.title.name.as_str()).collect();
        assert_eq!(names, ["The Quiet Harbour"]);
        assert_eq!(stats.genres, [("Drama".to_string(), 60 * 60)]);
        assert_eq!(stats.days.unwrap()[6], 0, "no Sunday");
        assert_eq!(stats.hours.unwrap()[21], 60 * 60);
    }

    #[test]
    fn the_year_in_review_holds_this_years_hours_top_title_genre_and_busiest_month() {
        let stats = plugin(vec![
            play(
                movie("Amber Fields", &["Drama"]),
                Some(at(2026, 3, 4, 20)),
                100,
            ),
            play(
                series("Glass Orchard", &["Comedy"]),
                Some(at(2026, 3, 9, 20)),
                40,
            ),
            play(
                series("Glass Orchard", &["Comedy"]),
                Some(at(2026, 7, 9, 20)),
                90,
            ),
            play(
                movie("Old Times", &["Western"]),
                Some(at(2025, 12, 30, 20)),
                900,
            ),
        ]);
        assert_eq!(
            stats.review,
            Review {
                year: 2026,
                seconds: 230 * 60,
                top_title: Some("Glass Orchard".into()),
                top_genre: Some("Comedy".into()),
                busiest_month: Some(3),
            }
        );
        assert_eq!(month_name(3), "March");
    }
}
