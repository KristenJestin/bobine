//! New episodes of the series the viewer follows, told to the desktop: what a check finds on the
//! server, and the `Notifier` port that shows it. The D-Bus side is an adapter behind the port
//! (`dbus`); the tests use a fake.

pub mod dbus;

use std::collections::{HashSet, VecDeque};
use std::future::poll_fn;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use crate::config::EpisodesSeen;
use bobine::provider::{Favourite, ImageRef, ItemId, MediaServer, NewEpisode, ProviderError};

/// How far before the last check's mark an episode not seen yet is still told: an episode is
/// dated when its file was made or downloaded, which may be well before the server scanned it.
pub const GRACE: i64 = 48 * 3_600 * 10_000_000;

/// How far past the local clock an episode's date may be and still move the mark: a server's
/// clock a little ahead is no reason to stop, but one file dated years ahead would push the mark
/// there and leave every real episode older than the window.
const AHEAD: i64 = 3_600 * 10_000_000;

/// How often the server is asked for new episodes while Bobine runs.
pub const EVERY: Duration = Duration::from_secs(15 * 60);

/// How many episodes a page of the server's holds.
const LATEST: u32 = 100;

/// A hard stop on the pages one check reads, whatever the server answers: two thousand episodes
/// added since the last check are already more than a library ever gains in a day.
const PAGES: u32 = 20;

/// How many series in progress make the followed ones, beside the favourites.
const IN_PROGRESS: u32 = 200;

/// The local clock, in ticks of 100 ns since 1970, as the episodes are dated.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| (since.as_nanos() / 100) as i64)
}

/// What a click on a notification opens: the episode, on its series' page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    /// The account it was told to (`<server id>/<user id>`): another one signed in since has no
    /// page for it.
    pub account: String,
    pub series: ItemId,
    pub season: ItemId,
    pub episode: ItemId,
}

/// What the desktop shows: built from the server's names and a local file only, never from an
/// address, which may carry a token.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    /// The series' name.
    pub summary: String,
    /// "S1 · E4 is available", "3 new episodes".
    pub body: String,
    /// The series' poster, written to the cache.
    pub icon: Option<PathBuf>,
    pub open: Open,
}

/// The port: whatever shows a notification to the desktop.
pub trait Notifier {
    /// Shows `notification`; a click on it comes back through `clicks`.
    fn notify(&mut self, notification: &Notification);
    /// Where the clicks come, whichever notification they were on.
    fn clicks(&self) -> Clicks;
}

/// The clicks on the notifications, from whatever thread hears them, to the window's, which
/// waits for them without asking again and again.
#[derive(Clone, Default)]
pub struct Clicks(Arc<Mutex<Inbox>>);

#[derive(Default)]
struct Inbox {
    heard: VecDeque<Open>,
    waiting: Option<Waker>,
}

impl Clicks {
    pub fn push(&self, open: Open) {
        let mut inbox = self.0.lock().unwrap();
        inbox.heard.push_back(open);
        if let Some(waker) = inbox.waiting.take() {
            waker.wake();
        }
    }

    /// The next click, oldest first.
    pub async fn next(&self) -> Open {
        poll_fn(|cx| {
            let mut inbox = self.0.lock().unwrap();
            match inbox.heard.pop_front() {
                Some(open) => Poll::Ready(open),
                None => {
                    inbox.waiting = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

/// The series that gained episodes since the last check: one per series, however many episodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Gained {
    pub series_title: String,
    pub body: String,
    pub poster: Option<ImageRef>,
    /// The first of its new episodes, by season and number: the one to watch first.
    pub series: ItemId,
    pub season: ItemId,
    pub episode: ItemId,
}

/// What a check found, and what it saw, which the next check starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub seen: EpisodesSeen,
    pub gained: Vec<Gained>,
}

/// The episodes of the series the viewer follows (in Favorites, or with an episode up next) that
/// the last check did not see, added after its mark less `GRACE`: an episode the server scanned
/// late, dated by its file, is still told once. With no `seen` (the first check of this account
/// ever) nothing is told, or a new install would flood the desktop: the check only keeps what it
/// saw for the next.
///
/// What is kept stays small: the episodes within `GRACE` of the newest mark. One request when
/// nothing new came in; the followed series are asked only when something did.
pub fn check(
    server: &dyn MediaServer,
    seen: Option<&EpisodesSeen>,
    now: i64,
) -> Result<Checked, ProviderError> {
    let latest = unplayed(server, seen.map(|seen| seen.mark), now)?;
    let newest = latest
        .iter()
        .map(|new| new.added)
        .filter(|added| *added <= now + AHEAD)
        .max();
    let mark = match (seen, newest) {
        (Some(seen), Some(newest)) => seen.mark.max(newest),
        (Some(seen), None) => seen.mark,
        (None, newest) => newest.unwrap_or(0),
    };
    // Carried forward, not replaced: an episode this check did not read (a page that shifted
    // while it was read, a list that came back empty) is still on the server.
    let mut added = seen.map(|seen| seen.added.clone()).unwrap_or_default();
    added.extend(
        latest
            .iter()
            .map(|new| (new.episode.item.id.0.clone(), new.added)),
    );
    added.retain(|_, at| *at > mark - GRACE);
    let now_seen = EpisodesSeen { mark, added };
    let checked = |gained| Checked {
        seen: now_seen,
        gained,
    };
    let Some(seen) = seen else {
        return Ok(checked(Vec::new()));
    };
    let fresh: Vec<NewEpisode> = latest
        .into_iter()
        .filter(|new| {
            new.added > seen.mark - GRACE
                && !new.episode.item.watched
                && !seen.added.contains_key(&new.episode.item.id.0)
        })
        .collect();
    if fresh.is_empty() {
        return Ok(checked(Vec::new()));
    }
    let followed = followed(server)?;
    let mut gained: Vec<(ItemId, Vec<NewEpisode>)> = Vec::new();
    for new in fresh {
        if !followed.contains(&new.episode.series) {
            continue;
        }
        let series = new.episode.series.clone();
        match gained.iter_mut().find(|(id, _)| *id == series) {
            Some((_, episodes)) => episodes.push(new),
            None => gained.push((series, vec![new])),
        }
    }
    Ok(checked(
        gained
            .into_iter()
            .map(|(_, episodes)| told(episodes))
            .collect(),
    ))
}

/// The episodes not played, each once, the newest added first, page after page down to the first
/// that reaches back to `mark` less `GRACE` (with no mark yet, the newest's not dated past
/// `AHEAD`), or the last, or `PAGES` of them.
fn unplayed(
    server: &dyn MediaServer,
    mark: Option<i64>,
    now: i64,
) -> Result<Vec<NewEpisode>, ProviderError> {
    let mut found: Vec<NewEpisode> = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();
    let mut floor = mark.map(|mark| mark - GRACE);
    for page in 0..PAGES {
        let start = page * LATEST;
        let read = server.new_episodes(start, LATEST)?;
        let floor = *floor.get_or_insert_with(|| {
            read.items
                .iter()
                .find(|new| new.added <= now + AHEAD)
                .map_or(i64::MIN, |new| new.added - GRACE)
        });
        let reached = read.items.last().is_some_and(|new| new.added <= floor);
        // A page that shifted between two reads gives an episode twice.
        found.extend(
            read.items
                .into_iter()
                .filter(|new| ids.insert(new.episode.item.id.0.clone())),
        );
        if reached || start + LATEST >= read.total {
            break;
        }
    }
    Ok(found)
}

/// The series in Favorites and those with an episode up next.
fn followed(server: &dyn MediaServer) -> Result<HashSet<ItemId>, ProviderError> {
    let mut followed: HashSet<ItemId> = server
        .favourites()?
        .into_iter()
        .filter_map(|favourite| match favourite {
            Favourite::Series(series) => Some(series.id),
            Favourite::Film(_) => None,
        })
        .collect();
    followed.extend(
        server
            .next_episodes(IN_PROGRESS)?
            .into_iter()
            .map(|episode| episode.series),
    );
    Ok(followed)
}

/// What the desktop is told of a series' new episodes.
fn told(mut episodes: Vec<NewEpisode>) -> Gained {
    episodes.sort_by_key(|new| (new.episode.season_number, new.episode.number));
    let first = &episodes[0];
    let body = match (
        episodes.len(),
        first.episode.season_number,
        first.episode.number,
    ) {
        (1, Some(season), Some(number)) => format!("S{season} · E{number} is available"),
        (1, _, _) => "A new episode is available".to_string(),
        (count, _, _) => format!("{count} new episodes"),
    };
    Gained {
        series_title: first.episode.series_title.clone(),
        body,
        poster: first.poster.clone(),
        series: first.episode.series.clone(),
        season: first.episode.season.clone(),
        episode: first.episode.item.id.clone(),
    }
}

#[cfg(test)]
pub mod fake {
    //! A notifier that shows nothing: it records what it is given; a test clicks through its
    //! `clicks`.

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Default, Clone)]
    pub struct FakeNotifier {
        pub shown: Rc<RefCell<Vec<Notification>>>,
        pub clicks: Clicks,
    }

    impl Notifier for FakeNotifier {
        fn notify(&mut self, notification: &Notification) {
            self.shown.borrow_mut().push(notification.clone());
        }

        fn clicks(&self) -> Clicks {
            self.clicks.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;

    /// Episodes the fake server added: series-1 is in Favorites, series-0 has one up next, and
    /// series-3 is neither.
    fn added(server: &FakeServer, episodes: &[(&str, i64)]) {
        server
            .state
            .lock()
            .unwrap()
            .added
            .extend(episodes.iter().map(|(id, at)| (ItemId((*id).into()), *at)));
    }

    /// What a check that marked `mark` and saw `episodes`, added when they say, keeps.
    fn seen(mark: i64, episodes: &[(&str, i64)]) -> EpisodesSeen {
        EpisodesSeen {
            mark,
            added: episodes
                .iter()
                .map(|(id, at)| (id.to_string(), *at))
                .collect(),
        }
    }

    /// A last check at `mark` that saw nothing.
    fn at(mark: i64) -> EpisodesSeen {
        seen(mark, &[])
    }

    /// The local clock of the tests: later than every date they add, but one.
    const NOW: i64 = T + 24 * HOUR;

    /// A check at `NOW`.
    fn check(
        server: &dyn MediaServer,
        seen: Option<&EpisodesSeen>,
    ) -> Result<Checked, ProviderError> {
        super::check(server, seen, NOW)
    }

    fn asks(server: &FakeServer) -> (u32, u32) {
        let state = server.state.lock().unwrap();
        (state.record.new_episode_asks, state.record.list_loads)
    }

    #[test]
    fn the_first_check_ever_tells_nothing_and_marks_the_newest() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e3", 100), ("series-0-s2-e5", 200)]);
        let checked = check(&server, None).unwrap();
        assert_eq!(
            checked,
            Checked {
                seen: seen(200, &[("series-0-s2-e5", 200), ("series-1-s1-e3", 100)]),
                gained: vec![]
            }
        );
    }

    #[test]
    fn a_new_episode_of_a_favourite_series_is_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 150)]);
        let checked = check(&server, Some(&at(100))).unwrap();
        assert_eq!(checked.seen, seen(150, &[("series-1-s1-e4", 150)]));
        assert_eq!(
            checked.gained,
            [Gained {
                series_title: "Les Cartographes".into(),
                body: "S1 · E4 is available".into(),
                poster: Some(ImageRef {
                    item: ItemId("series-1".into()),
                    kind: bobine::provider::ImageKind::Primary,
                    tag: "t1".into(),
                }),
                series: ItemId("series-1".into()),
                season: ItemId("series-1-s1".into()),
                episode: ItemId("series-1-s1-e4".into()),
            }]
        );
    }

    #[test]
    fn a_new_episode_of_a_series_in_progress_is_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-0-s3-e1", 150)]);
        let checked = check(&server, Some(&at(100))).unwrap();
        assert_eq!(checked.gained.len(), 1);
        assert_eq!(checked.gained[0].series, ItemId("series-0".into()));
    }

    #[test]
    fn a_series_neither_favourite_nor_in_progress_is_not_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-3-s1-e2", 150)]);
        let checked = check(&server, Some(&at(100))).unwrap();
        assert_eq!(
            checked,
            Checked {
                seen: seen(150, &[("series-3-s1-e2", 150)]),
                gained: vec![]
            }
        );
    }

    #[test]
    fn several_episodes_of_a_series_are_told_once_from_the_first() {
        let server = FakeServer::new(6, true);
        added(
            &server,
            &[
                ("series-1-s1-e3", 140),
                ("series-1-s1-e1", 120),
                ("series-1-s1-e2", 130),
            ],
        );
        let checked = check(&server, Some(&at(100))).unwrap();
        assert_eq!(checked.gained.len(), 1);
        assert_eq!(checked.gained[0].body, "3 new episodes");
        assert_eq!(checked.gained[0].episode, ItemId("series-1-s1-e1".into()));
    }

    #[test]
    fn an_episode_seen_by_the_last_check_is_not_told_again() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 100)]);
        let last = seen(100, &[("series-1-s1-e4", 100)]);
        let checked = check(&server, Some(&last)).unwrap();
        assert_eq!(
            checked,
            Checked {
                seen: last,
                gained: vec![]
            }
        );
    }

    #[test]
    fn an_episode_already_played_is_never_told() {
        let server = FakeServer::new(6, true);
        // series-0's first season is watched.
        added(&server, &[("series-0-s1-e2", 150)]);
        let checked = check(&server, Some(&at(100))).unwrap();
        assert_eq!(checked.gained, []);
    }

    #[test]
    fn nothing_new_costs_one_request() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 100)]);
        check(&server, Some(&seen(100, &[("series-1-s1-e4", 100)]))).unwrap();
        assert_eq!(asks(&server), (1, 0), "no list of favourites asked");
    }

    #[test]
    fn a_check_of_an_empty_server_marks_the_beginning() {
        let server = FakeServer::new(6, true);
        assert_eq!(check(&server, None).unwrap().seen, at(0));
        assert_eq!(check(&server, Some(&at(100))).unwrap().seen, at(100));
    }

    #[test]
    fn more_than_a_page_of_new_episodes_still_tells_the_followed_one() {
        let server = FakeServer::new(6, true);
        {
            let mut state = server.state.lock().unwrap();
            let other = ItemId("series-3".into());
            let template = state.episodes.iter().find(|e| e.series == other).unwrap();
            let template = template.clone();
            for n in 0..2 * LATEST as i64 + 50 {
                let mut episode = template.clone();
                episode.item.id = ItemId(format!("series-3-new-{n}"));
                episode.item.watched = false;
                state.added.push((episode.item.id.clone(), 200 + n));
                state.episodes.push(episode);
            }
        }
        added(&server, &[("series-1-s1-e4", 150)]);
        let checked = check(&server, Some(&at(100))).unwrap();
        let told: Vec<&ItemId> = checked.gained.iter().map(|g| &g.episode).collect();
        assert_eq!(told, [&ItemId("series-1-s1-e4".into())]);
    }

    const SECOND: i64 = 10_000_000;
    const HOUR: i64 = 3_600 * SECOND;
    /// 2025-09-14T08:30:05Z.
    const T: i64 = 1_757_838_605 * SECOND;

    fn told(checked: &Checked) -> Vec<&str> {
        checked
            .gained
            .iter()
            .map(|g| g.episode.0.as_str())
            .collect()
    }

    #[test]
    fn an_episode_dated_before_the_mark_but_seen_for_the_first_time_is_told_once() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e3", T + HOUR / 2)]);
        let first = check(&server, Some(&at(T - HOUR))).unwrap();
        assert_eq!(told(&first), ["series-1-s1-e3"]);
        // Scanned after that check, but dated by its file before it.
        added(&server, &[("series-0-s3-e1", T)]);
        let second = check(&server, Some(&first.seen)).unwrap();
        assert_eq!(told(&second), ["series-0-s3-e1"]);
        let third = check(&server, Some(&second.seen)).unwrap();
        assert_eq!(told(&third), Vec::<&str>::new(), "told once");
    }

    #[test]
    fn two_episodes_added_in_the_same_second_across_a_check_are_both_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e3", T + SECOND / 2)]);
        let first = check(&server, Some(&at(T - HOUR))).unwrap();
        assert_eq!(told(&first), ["series-1-s1-e3"]);
        added(&server, &[("series-0-s3-e1", T + SECOND / 10)]);
        let second = check(&server, Some(&first.seen)).unwrap();
        assert_eq!(told(&second), ["series-0-s3-e1"]);
    }

    #[test]
    fn an_episode_deleted_and_older_ones_left_never_come_back() {
        let server = FakeServer::new(6, true);
        added(
            &server,
            &[("series-1-s1-e3", T), ("series-0-s3-e1", T + HOUR)],
        );
        let first = check(&server, Some(&at(T - HOUR))).unwrap();
        assert_eq!(told(&first).len(), 2);
        server
            .state
            .lock()
            .unwrap()
            .added
            .retain(|(id, _)| id.0 != "series-0-s3-e1");
        let second = check(&server, Some(&first.seen)).unwrap();
        assert_eq!(told(&second), Vec::<&str>::new());
        assert_eq!(second.seen.mark, T + HOUR, "the mark does not go back");
        let third = check(&server, Some(&second.seen)).unwrap();
        assert_eq!(told(&third), Vec::<&str>::new());
    }

    #[test]
    fn an_episode_dated_more_than_two_days_before_the_mark_is_not_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e3", T - GRACE - SECOND)]);
        let checked = check(&server, Some(&at(T))).unwrap();
        assert_eq!(told(&checked), Vec::<&str>::new());
    }

    #[test]
    fn the_episodes_kept_are_those_of_the_two_days_before_the_mark() {
        let server = FakeServer::new(6, true);
        added(
            &server,
            &[
                ("series-1-s1-e3", T - GRACE - SECOND),
                ("series-1-s1-e4", T - GRACE + SECOND),
                ("series-0-s3-e1", T),
            ],
        );
        let checked = check(&server, None).unwrap();
        assert_eq!(
            checked.seen,
            seen(
                T,
                &[
                    ("series-0-s3-e1", T),
                    ("series-1-s1-e4", T - GRACE + SECOND)
                ]
            )
        );
    }

    /// `count` episodes of series-3, which nobody follows, added a second apart down from `from`.
    fn unfollowed(server: &FakeServer, count: i64, from: i64) {
        let mut state = server.state.lock().unwrap();
        let other = ItemId("series-3".into());
        let template = state.episodes.iter().find(|e| e.series == other).unwrap();
        let template = template.clone();
        for n in 0..count {
            let mut episode = template.clone();
            episode.item.id = ItemId(format!("series-3-new-{n}"));
            episode.item.watched = false;
            state
                .added
                .push((episode.item.id.clone(), from - n * SECOND));
            state.episodes.push(episode);
        }
    }

    #[test]
    fn an_episode_skipped_because_an_earlier_one_went_between_two_pages_is_not_told_again() {
        let server = FakeServer::new(6, true);
        unfollowed(&server, 150, T);
        // At place 100: the first of the second page.
        added(&server, &[("series-1-s1-e4", T - 99 * SECOND - SECOND / 2)]);
        let first = check(&server, Some(&at(T - HOUR))).unwrap();
        assert_eq!(told(&first), ["series-1-s1-e4"]);
        // Deleted once the first page is read: series-1-s1-e4 moves up to the first page's end.
        server.state.lock().unwrap().between_pages = Some(Box::new(|state| {
            state.added.retain(|(id, _)| id.0 != "series-3-new-0");
        }));
        let second = check(&server, Some(&first.seen)).unwrap();
        assert_eq!(told(&second), Vec::<&str>::new());
        let third = check(&server, Some(&second.seen)).unwrap();
        assert_eq!(told(&third), Vec::<&str>::new(), "not told again");
    }

    #[test]
    fn a_check_that_finds_no_episode_does_not_make_the_next_tell_them_again() {
        let server = FakeServer::new(6, true);
        added(
            &server,
            &[("series-1-s1-e3", T), ("series-0-s3-e1", T - HOUR)],
        );
        let first = check(&server, Some(&at(T - 2 * HOUR))).unwrap();
        assert_eq!(told(&first).len(), 2);
        let kept = std::mem::take(&mut server.state.lock().unwrap().added);
        let empty = check(&server, Some(&first.seen)).unwrap();
        assert_eq!(told(&empty), Vec::<&str>::new());
        server.state.lock().unwrap().added = kept;
        let after = check(&server, Some(&empty.seen)).unwrap();
        assert_eq!(told(&after), Vec::<&str>::new(), "not told again");
    }

    #[test]
    fn an_episode_read_twice_because_one_came_in_between_two_pages_is_told_as_one() {
        let server = FakeServer::new(6, true);
        unfollowed(&server, 151, T + SECOND);
        // Added on top once the first page is read.
        let top = server.state.lock().unwrap().added.remove(0);
        // At place 99: the last of the first page, then the first of the second.
        added(&server, &[("series-1-s1-e4", T - 98 * SECOND - SECOND / 2)]);
        server.state.lock().unwrap().between_pages =
            Some(Box::new(move |state| state.added.push(top)));
        let checked = check(&server, Some(&at(T - HOUR))).unwrap();
        assert_eq!(told(&checked), ["series-1-s1-e4"]);
        assert_eq!(checked.gained[0].body, "S1 · E4 is available");
    }

    #[test]
    fn an_episode_dated_in_the_future_does_not_move_the_mark() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-0-s3-e1", T + 5 * 365 * 24 * HOUR)]);
        let first = super::check(&server, Some(&at(T - HOUR)), T).unwrap();
        assert_eq!(told(&first), ["series-0-s3-e1"], "told once, as new");
        assert_eq!(first.seen.mark, T - HOUR);
        added(&server, &[("series-1-s1-e4", T)]);
        let second = super::check(&server, Some(&first.seen), T + HOUR).unwrap();
        assert_eq!(told(&second), ["series-1-s1-e4"]);
    }

    #[test]
    fn a_click_heard_before_anyone_waits_is_kept() {
        let clicks = Clicks::default();
        let open = |n: &str| Open {
            account: "server-1/user-1".into(),
            series: ItemId(n.into()),
            season: ItemId("s".into()),
            episode: ItemId("e".into()),
        };
        clicks.push(open("a"));
        clicks.push(open("b"));
        assert_eq!(zbus::block_on(clicks.next()), open("a"));
        assert_eq!(zbus::block_on(clicks.next()), open("b"));
    }
}
