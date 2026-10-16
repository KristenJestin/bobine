//! The Ctrl+K search palette and the page of a person it can lead to: the state around
//! `crate::search::Palette` (the field, the fades, the timers) and the calls to the server.

use std::time::Instant;

use bobine::provider::{ImageRef, ItemId, PersonHit, PersonTitles};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Focusable, ScrollHandle, Subscription, Task, Window, point, px};

use super::{Loadable, Shell};
use crate::input::TextInput;
use crate::motion::{self, Fade, Glide};
use crate::navigation::Screen;
use crate::search::{self, Hit, Key, PER_GROUP, Palette};

/// Whether the series page exists: until it does, a series found cannot be opened.
pub const SERIES_PAGE: bool = false;

pub struct SearchState {
    pub input: Entity<TextInput>,
    pub palette: Palette,
    /// Closed means fading out: the state goes once nothing shows any more.
    pub open: bool,
    pub fade: Fade,
    /// Where the highlighted row is, from the top of the list, and the height of the list.
    pub highlight: Glide,
    pub body: Glide,
    pub had_hits: bool,
    pub scroll: ScrollHandle,
    /// How far the list is scrolled, while the arrows scroll it.
    pub scrolling: Glide,
    wake: Option<Task<()>>,
    _typed: Subscription,
}

impl SearchState {
    /// The list is back at its top, for new results.
    fn rewind(&mut self) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.scrolling.jump(0.);
    }

    /// Scrolls, eased, so that the selected row shows whole in a list `view` tall.
    fn reveal(&mut self, view: f32, now: Instant) {
        let lines = self.palette.lines();
        let scrolled = -f32::from(self.scroll.offset().y);
        let wanted = search::scroll_to_show(&lines, self.palette.selected(), scrolled, view);
        self.scrolling.jump(scrolled);
        self.scrolling.set(wanted, now);
    }
}

/// What a person took part in.
pub struct PersonPage {
    pub id: ItemId,
    pub name: String,
    pub photo: Option<ImageRef>,
    pub titles: Loadable<PersonTitles>,
    pub scroll: ScrollHandle,
}

impl Shell {
    pub fn search_is_open(&self) -> bool {
        self.search.as_ref().is_some_and(|state| state.open)
    }

    /// Ctrl+K or the pill bar's field: opens the palette over the page, or closes it. Not on
    /// the login (no server yet) nor in the player. Says whether it did anything.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.server.is_none()
            || !matches!(
                self.navigator.current(),
                Screen::Home | Screen::Library | Screen::FilmDetail | Screen::Person
            )
        {
            return false;
        }
        if self.search_is_open() {
            self.close_search(window, cx);
            return true;
        }
        for page in &mut self.details {
            page.menus.close();
        }
        let input =
            cx.new(|cx| TextInput::new("Rechercher un film, une série, une personne", false, cx));
        let typed = cx.observe(&input, |shell, _, cx| shell.search_typed(cx));
        window.focus(&input.focus_handle(cx), cx);
        let now = Instant::now();
        let mut fade = Fade::new(motion::quick());
        fade.set(true, now);
        self.search = Some(SearchState {
            input,
            palette: Palette::default(),
            open: true,
            fade,
            highlight: Glide::new(0., motion::quick()),
            body: Glide::new(0., motion::quick()),
            had_hits: false,
            scroll: ScrollHandle::new(),
            scrolling: Glide::new(0., motion::quick()),
            wake: None,
            _typed: typed,
        });
        cx.notify();
        true
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &mut self.search else {
            return;
        };
        if !state.open {
            return;
        }
        state.open = false;
        state.wake = None;
        // The page under it gets its keys back at once, while the palette fades.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The field changed: the search goes out once the user stops typing.
    fn search_typed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.search else {
            return;
        };
        if !state.open {
            return;
        }
        let text = state.input.read(cx).text().to_string();
        let before = state.palette.due();
        state.palette.typed(&text, Instant::now());
        if state.palette.due() != before {
            self.schedule_search(cx);
        }
        cx.notify();
    }

    /// Wakes the window when the search is due, replacing any earlier wake.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.search else {
            return;
        };
        state.wake = state.palette.due().map(|due| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(due.saturating_duration_since(Instant::now()))
                    .await;
                this.update(cx, |shell, cx| shell.search_due(cx)).ok();
            })
        });
    }

    fn search_due(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.search else {
            return;
        };
        let Some(ask) = state.palette.ask(Instant::now()) else {
            self.schedule_search(cx);
            return;
        };
        state.wake = None;
        let asked = ask.clone();
        self.ask(
            cx,
            move |server| server.search(&ask.query, PER_GROUP),
            move |shell, answer, cx| {
                let answer = answer.map_err(|error| shell.failed(error, cx));
                if let Some(state) = &mut shell.search
                    && state.palette.answered(&asked, answer)
                {
                    state.rewind();
                }
            },
        );
    }

    /// A key while the palette is open: the arrows, Enter and Escape are its own.
    pub fn search_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.search_list_height(window);
        let Some(state) = &mut self.search else {
            return;
        };
        match state.palette.key(key) {
            Key::Moved => state.reveal(view, Instant::now()),
            Key::Open(hit) => self.open_hit(hit, window, cx),
            Key::Close => self.close_search(window, cx),
            Key::Ignored => {}
        }
        cx.notify();
    }

    /// The mouse is over the result `index`.
    pub fn search_hover(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.search
            && state.palette.selected() != index
        {
            state.palette.select(index);
            cx.notify();
        }
    }

    /// The tallest the list of results may be in this window.
    pub fn search_list_height(&self, window: &Window) -> f32 {
        let room = f32::from(window.viewport_size().height) - search::PANEL_TOP - search::CHROME;
        room.clamp(search::ROW_HEIGHT, search::LIST_MAX)
    }

    pub fn open_hit(&mut self, hit: Hit, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(hit, Hit::Series(_)) && !SERIES_PAGE {
            return;
        }
        self.close_search(window, cx);
        match hit {
            Hit::Film(film) => self.open_film(film.id, cx),
            Hit::Person(person) => self.open_person(person, cx),
            Hit::Series(series) => self.open_series(series.id, cx),
        }
    }

    pub fn open_person(&mut self, person: PersonHit, cx: &mut Context<Self>) {
        let id = person.id.clone();
        self.people.push(PersonPage {
            id: person.id,
            name: person.name,
            photo: person.photo,
            titles: Loadable::Loading,
            scroll: ScrollHandle::new(),
        });
        self.navigator.open(Screen::Person);
        let depth = self.people.len() - 1;
        let asked = id.clone();
        self.ask(
            cx,
            move |server| server.person_titles(&asked),
            move |shell, titles, cx| {
                let titles = match titles {
                    Ok(titles) => Loadable::Ready(titles),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                if let Some(page) = shell.people.get_mut(depth).filter(|page| page.id == id) {
                    page.titles = titles;
                }
            },
        );
        cx.notify();
    }
}
