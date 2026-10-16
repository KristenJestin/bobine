//! A card that answers the pointer: a poster, or a wide card of the Home. The pointer over its
//! picture brings a veil, a play button in the middle and, on a poster, the "Favorite",
//! "Watched" and "Add to playlist" buttons in a corner; a click anywhere else opens the title's
//! page. Each card is a view of its own, kept by the shell for each card drawn: the pointer
//! coming, going and moving over it redraws that card alone, not the page around it, however
//! long the rows.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use bobine::provider::{Episode, ItemId};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, ElementId, Entity, IntoElement, Pixels, Render, Rgba,
    SharedString, Stateful, WeakEntity, Window, canvas, div, px, rgb, rgba,
};

use super::home::Pick;
use crate::home_page::PlayTarget;
use crate::icons::{IconName, icon};
use crate::motion::{self, Fade};
use crate::poster_actions::{Action, Marks};
use crate::shell::Shell;
use crate::theme;

/// What a click on the card opens.
#[derive(Clone)]
pub enum Open {
    Film(ItemId),
    /// The series, and the season to open it on.
    Series(ItemId, Option<ItemId>),
    /// The title is read where the card's row holds it, when the card is clicked.
    Pick(Pick),
    /// The row around the card takes the click: nothing opens here. The episode tells this card
    /// from the one before it in the same place.
    Row(ItemId),
}

impl Open {
    /// The title the card stands for.
    fn item(&self) -> Option<&ItemId> {
        match self {
            Open::Film(id)
            | Open::Series(id, _)
            | Open::Row(id)
            | Open::Pick(Pick::Listed(_, id)) => Some(id),
            Open::Pick(Pick::Row(titles, at)) => titles.get(*at).map(|title| title.id()),
        }
    }
}

/// What the play button starts.
#[derive(Clone)]
pub enum Play {
    /// The title is read where the card's row holds it, when the button is pressed.
    Pick(Pick),
    Target(PlayTarget),
    /// An episode of the series page, which plays as the page does and comes back to it.
    Episode(Box<Episode>),
}

/// A button of the corner: what the title's list says, and what the button shows now.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub listed: bool,
    pub now: bool,
}

/// The buttons of a poster's corner.
#[derive(Clone)]
pub struct Quick {
    pub item: ItemId,
    pub my_list: Shown,
    pub watched: Shown,
}

impl Quick {
    /// The buttons of `item`, as its list says it stands and as the presses since left it.
    pub fn new(marks: &Marks, item: &ItemId, in_list: bool, watched: bool) -> Self {
        let shown = |action, listed| Shown {
            listed,
            now: marks.shown(item, action, listed),
        };
        Self {
            item: item.clone(),
            my_list: shown(Action::MyList, in_list),
            watched: shown(Action::Watched, watched),
        }
    }
}

/// Draws the card around the layer the pointer brings, which goes over its picture.
pub type Draw = Rc<dyn Fn(Stateful<Div>) -> Div>;

/// What the card shows and offers, given by the shell at each of its draws.
#[derive(Clone)]
pub struct Offer {
    pub open: Open,
    pub play: Play,
    pub quick: Option<Quick>,
    pub draw: Draw,
}

/// A card. Like the Home's header, it only reads the shell: it is drawn again with it, while the
/// shell never touches the card, which would make the card's every change redraw the shell.
pub struct Card {
    shell: WeakEntity<Shell>,
    /// Where the shell keeps what the card offers.
    name: ElementId,
    fade: Fade,
}

impl Card {
    /// How much the layer shows at `now`.
    #[cfg(test)]
    pub fn hovered(&self, now: Instant) -> f32 {
        self.fade.level(now)
    }
}

/// A card drawn: its view, what it offers, and the shell's draw that drew it last.
struct Kept {
    view: Entity<Card>,
    offer: Offer,
    drawn: u64,
}

/// The cards drawn: one view for each, forgotten once its card is no longer drawn.
#[derive(Default)]
pub struct Cards {
    views: HashMap<ElementId, Kept>,
    frame: u64,
}

impl Cards {
    /// A new draw of the shell begins: the cards not drawn by the last one go.
    pub fn new_frame(&mut self) {
        let last = self.frame;
        self.views.retain(|_, kept| kept.drawn == last);
        self.frame += 1;
    }

    /// The card `id`, showing and offering `offer`.
    pub fn view(&mut self, id: &ElementId, offer: Offer, cx: &mut Context<Shell>) -> Entity<Card> {
        let frame = self.frame;
        // Another title in the same place is another card: the pointer and the buttons of the
        // one before are not its own.
        if let Some(kept) = self.views.get_mut(id)
            && kept.offer.open.item() == offer.open.item()
        {
            kept.drawn = frame;
            kept.offer = offer;
            return kept.view.clone();
        }
        let shell = cx.weak_entity();
        let name = id.clone();
        let view = cx.new(|_| Card {
            shell,
            name,
            fade: Fade::new(motion::quick()),
        });
        let kept = Kept {
            view: view.clone(),
            offer,
            drawn: frame,
        };
        self.views.insert(id.clone(), kept);
        view
    }

    /// What the card `id` offers, as the shell's last draw gave it.
    pub fn offer(&self, id: &ElementId) -> Option<Offer> {
        self.views.get(id).map(|kept| kept.offer.clone())
    }

    #[cfg(test)]
    pub fn ids(&self) -> Vec<ElementId> {
        self.views.keys().cloned().collect()
    }

    #[cfg(test)]
    pub fn find(&self, id: &ElementId) -> Option<Entity<Card>> {
        self.views.get(id).map(|kept| kept.view.clone())
    }
}

impl Render for Card {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let offer = {
            let shell = shell.read(cx);
            #[cfg(any(test, feature = "harness"))]
            shell
                .draws
                .borrow_mut()
                .cards
                .push((self.name.clone(), now));
            shell.cards.offer(&self.name)
        };
        let Some(offer) = offer else {
            return div().into_any_element();
        };
        let level = self.fade.level(now);
        if self.fade.moving(now) {
            window.request_animation_frame();
        }
        // The buttons come with the pointer: unseen, they would take the clicks that open the
        // page.
        let shown = self.fade.visible(now);
        let play = shown.then(|| self.play_button(offer.play, window, cx));
        let quick = (offer.quick)
            .filter(|_| shown)
            .map(|quick| self.corner(quick, window, cx));
        let name = &self.name;
        let layer = div()
            .id("card-hover")
            .debug_selector(|| format!("card-hover {name}"))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            // A veil of black that comes with the pointer.
            .bg(rgba((0x66 as f32 * level) as u32))
            .on_hover(cx.listener(|card, hovered: &bool, _, cx| {
                card.fade.set(*hovered, Instant::now());
                cx.notify();
            }))
            .children(play.map(|play| play.opacity(level)))
            .children(quick.map(|quick| quick.opacity(level)));
        let shell = self.shell.clone();
        let open = offer.open;
        (offer.draw)(layer)
            .id("card")
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                let open = open.clone();
                shell
                    .update(cx, |shell, cx| match open {
                        Open::Film(id) => shell.open_film(id, cx),
                        Open::Series(id, season) => shell.open_series_on(id, season, cx),
                        Open::Pick(pick) => {
                            if let Some(title) = pick.title(shell.list.as_ref().map(|l| &l.grid)) {
                                shell.open_title(&title, cx);
                            }
                        }
                        Open::Row(_) => {}
                    })
                    .ok();
            })
            .into_any_element()
    }
}

impl Card {
    fn play_button(
        &self,
        play: Play,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let shell = self.shell.clone();
        theme::eased(
            "play",
            theme::Shades {
                rest: rgb(0xffffff),
                hover: rgb(0xe4e4e7),
                press: rgb(0xd4d4d8),
            },
            window,
            cx,
        )
        .debug_selector(|| format!("card-play {}", self.name))
        .size(px(48.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(0x000000))
        .cursor_pointer()
        // A click on the button plays; it never opens the page behind it.
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            let play = play.clone();
            shell
                .update(cx, |shell, cx| {
                    let target = match play {
                        Play::Episode(episode) => return shell.play_episode(*episode, window, cx),
                        Play::Target(target) => Some(target),
                        Play::Pick(pick) => pick
                            .title(shell.list.as_ref().map(|list| &list.grid))
                            .map(|title| PlayTarget::of(&title)),
                    };
                    if let Some(target) = target {
                        shell.play_from_home(target, window, cx);
                    }
                })
                .ok();
        })
        .child(icon(IconName::Play, 22.))
    }

    /// "Favorite", "Watched" and "Add to playlist", side by side in the top right corner.
    fn corner(&self, quick: Quick, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let my_list = self.quick_button(
            Action::MyList,
            &quick,
            (IconName::Plus, IconName::CheckCircle),
            window,
            cx,
        );
        let watched = self.quick_button(
            Action::Watched,
            &quick,
            (IconName::Check, IconName::Check),
            window,
            cx,
        );
        let add = self.playlist_button(quick.item.clone(), window, cx);
        div()
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .flex()
            .gap(px(6.))
            .child(my_list)
            .child(watched)
            .child(add)
    }

    /// "Add to playlist": the menu opens under it.
    fn playlist_button(
        &self,
        item: ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
        let drawn = bounds.clone();
        let shell = self.shell.clone();
        let button = theme::eased(
            "playlist",
            theme::Shades {
                rest: rgba(0x09090acc),
                hover: rgba(0x2e2e33ee),
                press: rgba(0x3a3a40ee),
            },
            window,
            cx,
        )
        .debug_selector(|| format!("card-playlist {}", self.name))
        .size(px(30.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        // Like the play button, it never opens the page behind it.
        .on_click(move |_, _, cx: &mut App| {
            cx.stop_propagation();
            let item = item.clone();
            let at = bounds.get();
            shell
                .update(cx, |shell, cx| shell.open_playlist_menu(item, at, cx))
                .ok();
        })
        .child(icon(IconName::ListPlus, 14.));
        div()
            .relative()
            .child(button)
            .child(
                canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .into_any_element()
    }

    /// A small round button, filled with the accent while it is on; the change eases.
    fn quick_button(
        &self,
        action: Action,
        quick: &Quick,
        (off, on): (IconName, IconName),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label, shown) = match action {
            Action::MyList => ("my-list", quick.my_list),
            Action::Watched => ("watched", quick.watched),
        };
        let fade =
            window.use_keyed_state(SharedString::from(format!("{label}-fade")), cx, |_, _| {
                Fade::starting_at(motion::quick(), shown.now)
            });
        let now = Instant::now();
        fade.update(cx, |fade, _| fade.set(shown.now, now));
        let fade = *fade.read(cx);
        let level = fade.level(now);
        if fade.moving(now) {
            window.request_animation_frame();
        }
        let dark: Rgba = rgba(0x09090acc);
        let shell = self.shell.clone();
        let item = quick.item.clone();
        let size = 14.;
        theme::eased(
            label,
            theme::Shades {
                rest: motion::mix(dark, theme::accent(), level),
                hover: motion::mix(rgba(0x2e2e33ee), theme::accent(), level),
                press: motion::mix(rgba(0x3a3a40ee), theme::accent(), level),
            },
            window,
            cx,
        )
        .debug_selector(|| format!("card-{label} {}", self.name))
        .size(px(30.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(motion::mix(theme::text(), theme::background(), level))
        .cursor_pointer()
        // Like the play button, it never opens the page behind it.
        .on_click(move |_, _, cx: &mut App| {
            cx.stop_propagation();
            let item = item.clone();
            shell
                .update(cx, |shell, cx| {
                    shell.press_poster(item, action, shown.listed, cx)
                })
                .ok();
        })
        .child(
            div()
                .relative()
                .size(px(size))
                .child(icon(off, size).absolute().inset_0().opacity(1. - level))
                .child(icon(on, size).absolute().inset_0().opacity(level)),
        )
        .into_any_element()
    }
}
