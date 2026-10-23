//! Context menus in a window (SPEC §10.9): opening by right-click (on the pane under the
//! pointer, without moving focus) or `ShowContextMenu`, keyboard navigation, running items and
//! the confirmation dialog. The engine is `crate::menu`.

use std::collections::HashMap;

use gpui::{
    div, prelude::*, px, Context, FontWeight, IntoElement, KeyDownEvent, Modifiers, Pixels, Point,
    Window,
};
use mt_config::menu::{self as reg, Trigger};
use mt_mux::ctl::Live;

use crate::actions::{resolve_action, ACTIONS};
use crate::app::{typed_text, AppWindow};
use crate::components::{Button, ButtonStyle, ContextMenu, Modal, ToastKind};
use crate::menu::nav::{MenuNav, NavKey, NavOutcome};
use crate::menu::{self, Builtin, Click, Facts, Invocation, MenuRow, Plan, RowKind};
use crate::metrics::{PALETTE_TOP_OFFSET, RADIUS_CONTROL, TERM_FONT, TITLE_BAR_HEIGHT};
use crate::mouse_input::hit_test;
use crate::overlay_stack::OverlayKind;
use crate::settings::Settings;
use crate::theme::Theme;
use crate::view::DismissExt;
use crate::{ipc_ops, mux_host};

/// An open context menu.
pub(crate) struct OpenMenu {
    pub menu: &'static str,
    pub facts: Facts,
    pub position: Point<Pixels>,
    pub nav: MenuNav,
}

/// An invocation waiting on the confirmation dialog.
pub(crate) struct PendingMenu {
    pub inv: Invocation,
    pub facts: Facts,
}

fn nav_key(ev: &KeyDownEvent, platform: crate::platform::Platform) -> Option<NavKey> {
    let m = &ev.keystroke.modifiers;
    if m.control || m.platform {
        return None;
    }
    Some(match ev.keystroke.key.as_str() {
        "up" => NavKey::Up,
        "down" => NavKey::Down,
        "left" => NavKey::Left,
        "right" => NavKey::Right,
        "home" => NavKey::Home,
        "end" => NavKey::End,
        "enter" => NavKey::Enter,
        "escape" => NavKey::Escape,
        "backspace" => NavKey::Backspace,
        _ => {
            let text = typed_text(ev, platform)?;
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            NavKey::Char(c)
        }
    })
}

/// Chip text per row id, from the rows' actions and the live keymap.
fn menu_chips(settings: &Settings, rows: &[MenuRow], out: &mut HashMap<String, String>) {
    for r in rows {
        if let Some(origin) = crate::menu::shell::origin_of(&r.id) {
            out.insert(r.id.clone(), origin.to_string());
        }
        if let Some(chip) = r
            .chip_action
            .as_deref()
            .and_then(|a| ACTIONS.iter().find(|d| d.name == a))
            .and_then(|d| settings.chip(d))
        {
            out.insert(r.id.clone(), chip);
        }
        if let RowKind::Submenu(kids) = &r.kind {
            menu_chips(settings, kids, out);
        }
    }
}

impl AppWindow {
    /// Facts of `pane` (a pane of this window), with live facts from this window's terminals.
    pub(crate) fn menu_facts(&self, pane: u64, cx: &mut Context<Self>) -> Option<Facts> {
        let lives: HashMap<u64, Live> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .filter_map(|v| {
                v.session
                    .as_ref()
                    .map(|s| (v.pane, ipc_ops::live_of(&s.term)))
            })
            .collect();
        let rows = mux_host::host(cx)
            .mux
            .rows(&|id| lives.get(&id).cloned().unwrap_or_default());
        let term = self
            .tabs
            .iter()
            .find_map(|t| t.pane_view(pane))
            .and_then(|v| v.session.as_ref())
            .map(|s| s.term.clone());
        Facts::for_pane(&self.cfg, &rows, pane, term.as_deref())
    }

    /// The click facts of a right-click at `position` over `pane`.
    fn click_facts(&mut self, pane: u64, position: Point<Pixels>) -> Click {
        let Some(session) = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.pane_view(pane))
            .and_then(|v| v.session.as_ref())
        else {
            return Click::default();
        };
        let snap = session.snapshot.clone();
        let hit = hit_test(
            (f32::from(position.x), f32::from(position.y)),
            session.grid_origin.get(),
            (self.cell.width, self.cell.height),
            (snap.cols, snap.rows),
        );
        let marks = session.term.prompt_marks();
        let abs = (snap.history_size + hit.row).saturating_sub(snap.display_offset);
        let on_prompt = marks.contains(&abs);
        let in_output = !on_prompt && marks.iter().any(|&m| m < abs);
        let link = self.link_in(pane, hit.row, hit.col).map(|l| l.target);
        // Columns of the row text skip wide-char spacers, like `row_text`.
        let cells = snap.row(hit.row);
        let col = cells[..hit.col.min(cells.len())]
            .iter()
            .filter(|c| !c.flags.has(mt_term::CellFlags::WIDE_SPACER))
            .count();
        Click {
            link,
            line: Some((snap.row_text(hit.row), col)),
            on_prompt,
            in_output,
        }
    }

    /// Opens `menu` for `facts` at `position`. Nothing opens when it has no rows.
    pub(crate) fn open_menu(
        &mut self,
        menu: &'static str,
        facts: Facts,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let rows = menu::resolve(&self.cfg, menu, &facts);
        if rows.is_empty() {
            return;
        }
        let max_rows = self
            .cfg
            .context_menu
            .get(menu)
            .map_or(20, |m| m.max_rows as usize);
        self.mouse.hover = None;
        self.open_menu = Some(OpenMenu {
            menu,
            facts,
            position,
            nav: MenuNav::new(rows, max_rows),
        });
        cx.notify();
    }

    /// A right-button press over `pane` (SPEC §10.9.2 triggers). Returns whether it was used.
    pub(crate) fn pane_right_click(
        &mut self,
        pane: u64,
        position: Point<Pixels>,
        mods: &Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.overlays.is_empty() || self.open_menu.is_some() {
            return false;
        }
        match reg::viewport_trigger(&self.cfg) {
            Trigger::None => false,
            Trigger::Paste => {
                self.focus_pane(pane, window, cx);
                self.paste(window, cx);
                true
            }
            Trigger::ShiftRightClick if !mods.shift => false,
            Trigger::RightClick | Trigger::ShiftRightClick => {
                let click = self.click_facts(pane, position);
                let Some(facts) = self.menu_facts(pane, cx) else {
                    return false;
                };
                let facts = facts.with_click(&click, &self.cfg.selection.word_chars);
                self.open_menu("viewport", facts, position, cx);
                true
            }
        }
    }

    /// Sets (or with the ring, clears) the manual color of `tab` from a swatch id.
    pub(crate) fn pick_tab_color(&mut self, tab: u64, swatch: &str, cx: &mut Context<Self>) {
        let color = crate::tab_color::swatches(&self.cfg.tab_bar.palette)
            .into_iter()
            .find(|s| s.id == swatch)
            .map(|s| s.color);
        if let Some(color) = color {
            self.set_tab_color(tab, color, cx);
        }
    }

    /// Sets (`None` clears) the manual color of `tab`; the mux reports the change.
    pub(crate) fn set_tab_color(
        &mut self,
        tab: u64,
        color: Option<mt_mux::TabColor>,
        cx: &mut Context<Self>,
    ) {
        let _ = mux_host::host(cx).mux.set_tab_color(tab, color);
        cx.notify();
    }

    /// Right-click on a tab header.
    pub(crate) fn open_tab_menu(
        &mut self,
        tab: u64,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if reg::trigger(&self.cfg, "tab") == Trigger::None {
            return;
        }
        let Some(pane) = self.tabs.iter().find(|t| t.id == tab).map(|t| t.pane) else {
            return;
        };
        if let Some(mut facts) = self.menu_facts(pane, cx) {
            facts.manual_color = mux_host::host(cx).mux.tab(tab).and_then(|t| t.color);
            self.open_menu("tab", facts, position, cx);
        }
    }

    /// `ShowContextMenu(id)`: the menu for the focused pane (at its cursor) or the active tab.
    pub(crate) fn show_context_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(def) = reg::menu(id) else {
            self.toast(ToastKind::Warning, format!("Unknown menu \"{id}\""), cx);
            return;
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        let Some(mut facts) = self.menu_facts(pane, cx) else {
            return;
        };
        facts.manual_color = mux_host::host(cx).mux.tab(facts.tab).and_then(|t| t.color);
        let position = match (def.id, self.live()) {
            ("viewport", Some(s)) => {
                let (x, y) = s.grid_origin.get();
                let c = s.snapshot.cursor;
                gpui::point(
                    px(x + (c.col as f32 + 1.0) * self.cell.width),
                    px(y + (c.row as f32 + 1.0) * self.cell.height),
                )
            }
            _ => gpui::point(px(16.), px(TITLE_BAR_HEIGHT + 32.)),
        };
        let facts = if def.id == "viewport" {
            facts.with_click(&Click::default(), &self.cfg.selection.word_chars)
        } else {
            facts
        };
        self.open_menu(def.id, facts, position, cx);
    }

    /// Keys while a menu is open. Returns whether the menu took the key.
    pub(crate) fn menu_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let platform = self.platform;
        let Some(open) = self.open_menu.as_mut() else {
            return false;
        };
        let Some(key) = nav_key(ev, platform) else {
            return true;
        };
        match open.nav.key(key) {
            NavOutcome::Activate(id) => self.activate_menu_row(id, window, cx),
            NavOutcome::Close => {
                self.open_menu = None;
                cx.notify();
            }
            NavOutcome::Moved | NavOutcome::Ignored => cx.notify(),
        }
        true
    }

    fn activate_menu_row(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open_menu.take() else {
            return;
        };
        cx.notify();
        if let Some(swatch) = id.strip_prefix("color:") {
            self.pick_tab_color(open.facts.tab, swatch, cx);
            return;
        }
        self.invoke_menu(open.menu, &id, open.facts, window, cx);
    }

    /// Plans `menu/item` and runs it, through the confirmation dialog when needed.
    pub(crate) fn invoke_menu(
        &mut self,
        menu: &str,
        item: &str,
        facts: Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match menu::plan(&self.cfg, menu, item, &facts) {
            Err(e) => self.toast(ToastKind::Warning, e.to_string(), cx),
            Ok(inv) if inv.confirm.is_some() => {
                self.menu_confirm = Some(PendingMenu { inv, facts });
                self.open_overlay(OverlayKind::MenuConfirm, window, cx);
            }
            Ok(inv) => self.run_invocation(inv, facts, window, cx),
        }
    }

    /// `InvokeMenuItem("menu/item")`: runs an item against the focused pane.
    pub(crate) fn invoke_menu_address(
        &mut self,
        addr: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (menu, item) = match menu::parse_address(addr) {
            Ok(x) => x,
            Err(e) => return self.toast(ToastKind::Warning, e.to_string(), cx),
        };
        let Some(pane) = self.focused_pane() else {
            return;
        };
        if let Some(facts) = self.menu_facts(pane, cx) {
            let facts = if menu == "viewport" {
                facts.with_click(&Click::default(), &self.cfg.selection.word_chars)
            } else {
                facts
            };
            self.invoke_menu(menu, item, facts, window, cx);
        }
    }

    pub(crate) fn confirm_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = self.menu_confirm.take();
        self.close_overlay(OverlayKind::MenuConfirm, true, window, cx);
        if let Some(p) = pending {
            self.run_invocation(p.inv, p.facts, window, cx);
        }
    }

    fn run_invocation(
        &mut self,
        inv: Invocation,
        facts: Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match inv.plan {
            Plan::Builtin(b) => self.run_gui_builtin(b, &facts, window, cx),
            Plan::Action(_) => self.run_menu_action(&inv, window, cx),
            _ => cx.defer(move |cx| {
                if let Err(e) = mux_host::menu_execute(cx, &inv) {
                    mux_host::toast_all(cx, ToastKind::Error, e.message);
                }
            }),
        }
    }

    /// A catalogue action: it acts on the focused pane, so the target pane is focused first.
    pub(crate) fn run_menu_action(
        &mut self,
        inv: &Invocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Plan::Action(a) = &inv.plan else {
            return;
        };
        match resolve_action(&a.name, &a.arg) {
            Ok(action) => {
                self.focus_pane(inv.pane, window, cx);
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            Err(e) => self.toast(ToastKind::Warning, e, cx),
        }
    }

    /// A built-in row, on the pane (or tab) the menu was opened for.
    pub(crate) fn run_gui_builtin(
        &mut self,
        b: Builtin,
        facts: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = facts.pane;
        let term = self
            .tabs
            .iter()
            .find_map(|t| t.pane_view(pane))
            .and_then(|v| v.session.as_ref())
            .map(|s| s.term.clone());
        match b {
            Builtin::Copy => {
                let trim = self.cfg.selection.trim_trailing_whitespace;
                if let Some(text) = term.and_then(|t| t.selection_text(trim)) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
            }
            Builtin::OpenLink => {
                if let Some(l) = facts.values.get("link").map(str::to_string) {
                    self.open_link(&l, cx);
                }
            }
            Builtin::CopyLink => {
                if let Some(l) = facts.values.get("link").map(str::to_string) {
                    self.copy_link(&l, cx);
                }
            }
            Builtin::Paste => {
                self.focus_pane(pane, window, cx);
                self.paste(window, cx);
            }
            Builtin::SelectAll => {
                self.focus_pane(pane, window, cx);
                self.select_all(cx);
            }
            Builtin::Search => {
                self.focus_pane(pane, window, cx);
                self.search_selection(window, cx);
            }
            Builtin::ClearScrollback => {
                self.focus_pane(pane, window, cx);
                self.clear_scrollback(cx);
            }
            Builtin::SaveAsSnippet => {
                self.focus_pane(pane, window, cx);
                self.save_selection_as_snippet(window, cx);
            }
            Builtin::ToggleRecording => {
                self.focus_pane(pane, window, cx);
                self.toggle_recording(window, cx);
            }
            Builtin::Zoom => self.toggle_zoom_of(pane, window, cx),
            Builtin::Swap(d) => self.swap_pane_of(pane, d, window, cx),
            Builtin::Rotate => self.rotate_panes_of(pane, window, cx),
            Builtin::Split(d) => {
                self.split_at(pane, d, None, window, cx);
            }
            Builtin::ClosePane => self.request_close_pane(pane, window, cx),
            Builtin::CloseTab => self.request_close_tab(facts.tab, window, cx),
            Builtin::CloseOtherTabs => self.close_other_tabs(facts.tab, window, cx),
            Builtin::RenameTab => {
                if let Some(index) = self.tabs.iter().position(|t| t.id == facts.tab) {
                    self.activate_tab(index, cx);
                }
                self.begin_rename(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn render_open_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let open = self.open_menu.as_ref()?;
        let mut chips = HashMap::new();
        menu_chips(cx.global::<Settings>(), &open.nav.rows, &mut chips);
        let entity = cx.entity();
        let (e1, e2, e3, e4, e5) = (
            entity.clone(),
            entity.clone(),
            entity.clone(),
            entity.clone(),
            entity,
        );
        Some(
            ContextMenu::new(
                gpui::SharedString::from(format!("{}-menu", open.menu)),
                open.position,
                open.nav.clone(),
            )
            .chips(chips)
            .palette(self.cfg.tab_bar.palette.clone())
            .on_dismiss(move |_, app| {
                e1.update(app, |this, cx| {
                    this.open_menu = None;
                    cx.notify();
                });
            })
            .on_activate(move |id, window, app| {
                e2.update(app, |this, cx| this.activate_menu_row(id, window, cx));
            })
            .on_hover(move |i, _, app| {
                e3.update(app, |this, cx| {
                    if let Some(m) = this.open_menu.as_mut() {
                        if m.nav.highlight != i {
                            m.nav.hover(i);
                            cx.notify();
                        }
                    }
                });
            })
            .on_hover_child(move |j, _, app| {
                e4.update(app, |this, cx| {
                    if let Some(m) = this.open_menu.as_mut() {
                        m.nav.hover_child(j);
                        cx.notify();
                    }
                });
            })
            .on_scroll(move |d, _, app| {
                e5.update(app, |this, cx| {
                    if let Some(m) = this.open_menu.as_mut() {
                        m.nav.scroll_by(d);
                        cx.notify();
                    }
                });
            }),
        )
    }

    pub(crate) fn menu_confirm_modal(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let p = self.menu_confirm.as_ref()?;
        let text = menu::plan_text(&p.inv.plan);
        let lines = text.lines().count();
        let preview: String = text.lines().take(8).collect::<Vec<_>>().join("\n");
        let more = lines.saturating_sub(8);
        let what = match p.inv.kind {
            Some(mt_config::menu::Kind::Send) => format!("Send this text to pane {}?", p.inv.pane),
            Some(mt_config::menu::Kind::Spawn) => "Run this command in a new pane?".to_string(),
            Some(mt_config::menu::Kind::Ctl) => "Run this microterm ctl command?".to_string(),
            _ => format!("Run {}/{}?", p.inv.menu, p.inv.item),
        };
        let reason = p.inv.confirm.clone().unwrap_or_default();
        Some(
            Modal::new("menu-confirm-modal")
                .width(px(480.))
                .top(px(TITLE_BAR_HEIGHT + PALETTE_TOP_OFFSET))
                .on_dismiss(cx.listener_for_dismiss())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(what))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.text_muted)
                                .child(format!("Confirmation required: {reason}.")),
                        )
                        .child(
                            div()
                                .p_2()
                                .rounded(px(RADIUS_CONTROL))
                                .border_1()
                                .border_color(t.border)
                                .bg(t.background)
                                .font_family(TERM_FONT)
                                .text_size(px(12.))
                                .overflow_hidden()
                                .child(preview)
                                .when(more > 0, |el| {
                                    el.child(
                                        div()
                                            .text_color(t.text_placeholder)
                                            .child(format!("… {more} more lines")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .justify_end()
                                .child(Button::new("menu-confirm-cancel", "Cancel").on_click(
                                    cx.listener(|this, _, w, cx| this.dismiss_top(w, cx)),
                                ))
                                .child(
                                    Button::new("menu-confirm-run", "Run")
                                        .style(ButtonStyle::Filled)
                                        .on_click(
                                            cx.listener(|this, _, w, cx| this.confirm_menu(w, cx)),
                                        ),
                                ),
                        ),
                ),
        )
    }
}
