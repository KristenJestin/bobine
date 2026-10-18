//! One window at a time. `bobine open` and `bobine play` hand what they ask to the window already
//! running, through a Unix socket in the user's runtime folder; with none there, the command
//! opens the window itself, which then listens on it.
//!
//! Not MPRIS's `OpenUri`: the window takes `org.mpris.MediaPlayer2.bobine` only once a title
//! plays (nothing shows in the bar widget before), so a window that only browses could not be
//! reached through it. The socket needs no bus, belongs to the user alone (`XDG_RUNTIME_DIR` is
//! theirs, mode 0700), and a test makes its own in a temporary folder.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::future::poll_fn;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

use bobine::provider::ItemId;

use crate::film_page::From;

/// How long a command has to say what it asks.
const READ_FOR: Duration = Duration::from_secs(1);

/// Longer than any address.
const LINE_AT_MOST: u64 = 4096;

/// What a page is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Series,
    /// An episode's page is its series', on its season.
    Episode,
}

/// What a command asks the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Open(Kind, ItemId),
    /// A film, an episode, or a series' next episode, from where the viewer stopped or from the
    /// start.
    Play(ItemId, From),
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Series => "series",
            Kind::Episode => "episode",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [Kind::Movie, Kind::Series, Kind::Episode]
            .into_iter()
            .find(|kind| kind.word() == word)
    }
}

/// Whether `id` can stand in an address: something, with no separator nor space in it.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.contains(|c: char| c == '/' || c.is_whitespace() || c.is_control())
}

impl Request {
    /// The request as a `bobine:` address, the one line the socket carries.
    pub fn address(&self) -> String {
        match self {
            Request::Open(kind, id) => format!("bobine:open/{}/{}", kind.word(), id.0),
            Request::Play(id, From::Resume) => format!("bobine:play/{}", id.0),
            Request::Play(id, From::Beginning) => format!("bobine:play-from-start/{}", id.0),
        }
    }

    pub fn from_address(address: &str) -> Option<Self> {
        let (verb, rest) = address.strip_prefix("bobine:")?.split_once('/')?;
        let request = match verb {
            "open" => {
                let (kind, id) = rest.split_once('/')?;
                Request::Open(Kind::from_word(kind)?, ItemId(id.into()))
            }
            "play" => Request::Play(ItemId(rest.into()), From::Resume),
            "play-from-start" => Request::Play(ItemId(rest.into()), From::Beginning),
            _ => return None,
        };
        let (Request::Open(_, id) | Request::Play(id, _)) = &request;
        valid_id(&id.0).then_some(request)
    }
}

/// `$XDG_RUNTIME_DIR/bobine.sock`. A relative `XDG_RUNTIME_DIR` is ignored, as the XDG
/// specification says; without one, no window can be reached and each command opens its own.
pub fn socket_path(runtime_dir: Option<OsString>) -> Option<PathBuf> {
    let dir = PathBuf::from(runtime_dir?);
    dir.is_absolute().then(|| dir.join("bobine.sock"))
}

/// Hands `request` to the window listening at `path`: false when none listens there.
pub fn hand_off(path: &Path, request: &Request) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else {
        return false;
    };
    writeln!(stream, "{}", request.address()).is_ok()
}

/// What the commands asked, from the thread that hears them to the window's, which waits for
/// them without asking again and again.
#[derive(Clone, Default)]
pub struct Inbox(Arc<Mutex<Queue>>);

#[derive(Default)]
struct Queue {
    heard: VecDeque<Request>,
    waiting: Option<Waker>,
}

impl Inbox {
    pub fn push(&self, request: Request) {
        let mut queue = self.0.lock().unwrap();
        queue.heard.push_back(request);
        if let Some(waker) = queue.waiting.take() {
            waker.wake();
        }
    }

    /// The next request, oldest first.
    pub async fn next(&self) -> Request {
        poll_fn(|cx| {
            let mut queue = self.0.lock().unwrap();
            match queue.heard.pop_front() {
                Some(request) => Poll::Ready(request),
                None => {
                    queue.waiting = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

/// Listens at `path` for the window. `None` when another window already listens there (this one
/// opens all the same, out of the commands' reach), or when the socket cannot be made.
pub fn listen(path: &Path) -> Option<Inbox> {
    if UnixStream::connect(path).is_ok() {
        return None;
    }
    // What is left there is the socket of a window that ended.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).ok()?;
    let inbox = Inbox::default();
    let heard = inbox.clone();
    std::thread::Builder::new()
        .name("bobine-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(request) = read_request(stream) {
                    heard.push(request);
                }
            }
        })
        .ok()?;
    Some(inbox)
}

/// The one line a command sends; a client that says nothing is not waited for long.
fn read_request(stream: UnixStream) -> Option<Request> {
    stream.set_read_timeout(Some(READ_FOR)).ok()?;
    let mut line = String::new();
    BufReader::new(stream.take(LINE_AT_MOST))
        .read_line(&mut line)
        .ok()?;
    Request::from_address(line.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    fn movie() -> Request {
        Request::Open(Kind::Movie, ItemId("film-7".into()))
    }

    /// What `inbox` heard, once it heard `count` requests or five seconds went by.
    fn heard(inbox: &Inbox, count: usize) -> Vec<Request> {
        let start = std::time::Instant::now();
        while inbox.0.lock().unwrap().heard.len() < count && start.elapsed() < WAIT {
            std::thread::sleep(Duration::from_millis(20));
        }
        inbox.0.lock().unwrap().heard.drain(..).collect()
    }

    #[test]
    fn every_request_goes_through_its_address_and_back() {
        let requests = [
            movie(),
            Request::Open(Kind::Series, ItemId("series-0".into())),
            Request::Open(Kind::Episode, ItemId("series-0-s2-e4".into())),
            Request::Play(ItemId("film-0".into()), From::Resume),
            Request::Play(ItemId("film-0".into()), From::Beginning),
        ];
        for request in requests {
            assert_eq!(Request::from_address(&request.address()), Some(request));
        }
        assert_eq!(movie().address(), "bobine:open/movie/film-7");
        assert_eq!(
            Request::Play(ItemId("film-0".into()), From::Resume).address(),
            "bobine:play/film-0"
        );
    }

    #[test]
    fn an_address_that_names_nothing_is_refused() {
        for address in [
            "",
            "bobine:",
            "bobine:open/movie/",
            "bobine:open/album/a1",
            "bobine:open/movie/a/b",
            "bobine:play/",
            "bobine:rewind/film-0",
            "http:open/movie/film-0",
        ] {
            assert_eq!(Request::from_address(address), None, "{address}");
        }
    }

    #[test]
    fn the_socket_is_in_the_runtime_folder_and_only_an_absolute_one() {
        assert_eq!(
            socket_path(Some("/run/user/1000".into())),
            Some(PathBuf::from("/run/user/1000/bobine.sock"))
        );
        assert_eq!(socket_path(Some("relative".into())), None);
        assert_eq!(socket_path(None), None);
    }

    #[test]
    fn open_movie_with_a_window_running_is_handed_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine.sock");
        let inbox = listen(&path).expect("the first window listens");
        assert!(hand_off(&path, &movie()));
        assert_eq!(heard(&inbox, 1), [movie()]);
    }

    #[test]
    fn play_with_no_window_running_finds_none_to_hand_it_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine.sock");
        let play = Request::Play(ItemId("film-0".into()), From::Resume);
        assert!(!hand_off(&path, &play));
        // A socket left by a window that ended is no window either.
        drop(UnixListener::bind(&path).unwrap());
        assert!(!hand_off(&path, &play));
    }

    #[test]
    fn a_socket_left_by_a_window_that_ended_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine.sock");
        drop(UnixListener::bind(&path).unwrap());
        let inbox = listen(&path).expect("the new window listens");
        assert!(hand_off(&path, &movie()));
        assert_eq!(heard(&inbox, 1), [movie()]);
    }

    #[test]
    fn a_second_window_leaves_the_socket_to_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine.sock");
        let first = listen(&path).expect("the first window listens");
        assert!(listen(&path).is_none());
        assert!(hand_off(&path, &movie()));
        assert_eq!(heard(&first, 1), [movie()]);
    }

    #[test]
    fn a_line_that_is_no_request_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine.sock");
        let inbox = listen(&path).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.write_all(b"rm -rf /\n").unwrap();
        drop(stream);
        assert!(hand_off(&path, &movie()));
        assert_eq!(heard(&inbox, 1), [movie()]);
    }
}
