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

use bobine::provider::{Favourite, ImageRef, ItemId, MediaServer, NewEpisode, ProviderError};

/// How often the server is asked for new episodes while Bobine runs.
pub const EVERY: Duration = Duration::from_secs(15 * 60);

/// How many episodes a page of the server's holds.
const LATEST: u32 = 100;

/// A hard stop on the pages one check reads, whatever the server answers: two thousand episodes
/// added since the last check are already more than a library ever gains in a day.
const PAGES: u32 = 20;

/// How many series in progress make the followed ones, beside the favourites.
const IN_PROGRESS: u32 = 200;

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

/// What a check found, and the newest episode it saw, which the next check starts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub seen: i64,
    pub gained: Vec<Gained>,
}

/// The episodes added since `seen` to the series the viewer follows: in Favorites, or with an
/// episode up next. With no `seen` (the first check of this account ever) nothing is told, or a
/// new install would flood the desktop: the check only marks where the next starts from.
///
/// One request when nothing new came in; the followed series are asked only when something did.
pub fn check(server: &dyn MediaServer, seen: Option<i64>) -> Result<Checked, ProviderError> {
    let latest = unplayed(server, seen.unwrap_or(i64::MAX))?;
    let newest = latest.iter().map(|new| new.added).max();
    let Some(seen) = seen else {
        return Ok(Checked {
            seen: newest.unwrap_or(0),
            gained: Vec::new(),
        });
    };
    let fresh: Vec<NewEpisode> = latest
        .into_iter()
        .filter(|new| new.added > seen && !new.episode.item.watched)
        .collect();
    let checked = |gained| Checked {
        seen: newest.map_or(seen, |newest| newest.max(seen)),
        gained,
    };
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

/// The episodes not played, the newest added first, page after page down to the first that
/// reaches back to `floor`, or the last, or `PAGES` of them.
fn unplayed(server: &dyn MediaServer, floor: i64) -> Result<Vec<NewEpisode>, ProviderError> {
    let mut found = Vec::new();
    for page in 0..PAGES {
        let start = page * LATEST;
        let read = server.new_episodes(start, LATEST)?;
        let reached = read.items.last().is_some_and(|new| new.added <= floor);
        found.extend(read.items);
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
                seen: 200,
                gained: vec![]
            }
        );
    }

    #[test]
    fn a_new_episode_of_a_favourite_series_is_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 150)]);
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(checked.seen, 150);
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
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(checked.gained.len(), 1);
        assert_eq!(checked.gained[0].series, ItemId("series-0".into()));
    }

    #[test]
    fn a_series_neither_favourite_nor_in_progress_is_not_told() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-3-s1-e2", 150)]);
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(
            checked,
            Checked {
                seen: 150,
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
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(checked.gained.len(), 1);
        assert_eq!(checked.gained[0].body, "3 new episodes");
        assert_eq!(checked.gained[0].episode, ItemId("series-1-s1-e1".into()));
    }

    #[test]
    fn an_episode_added_before_the_last_check_is_not_told_again() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 100)]);
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(
            checked,
            Checked {
                seen: 100,
                gained: vec![]
            }
        );
    }

    #[test]
    fn an_episode_already_played_is_never_told() {
        let server = FakeServer::new(6, true);
        // series-0's first season is watched.
        added(&server, &[("series-0-s1-e2", 150)]);
        let checked = check(&server, Some(100)).unwrap();
        assert_eq!(checked.gained, []);
    }

    #[test]
    fn nothing_new_costs_one_request() {
        let server = FakeServer::new(6, true);
        added(&server, &[("series-1-s1-e4", 100)]);
        check(&server, Some(100)).unwrap();
        assert_eq!(asks(&server), (1, 0), "no list of favourites asked");
    }

    #[test]
    fn a_check_of_an_empty_server_marks_the_beginning() {
        let server = FakeServer::new(6, true);
        assert_eq!(check(&server, None).unwrap().seen, 0);
        assert_eq!(check(&server, Some(100)).unwrap().seen, 100);
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
        let checked = check(&server, Some(100)).unwrap();
        let told: Vec<&ItemId> = checked.gained.iter().map(|g| &g.episode).collect();
        assert_eq!(told, [&ItemId("series-1-s1-e4".into())]);
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
