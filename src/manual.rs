//! Manual pointer routing: hover/click detection from raw pointer position
//! and press/release events, bypassing egui hit-testing entirely.
//!
//! Only proven-live signals are used: `pointer_hover_pos`,
//! `button_down`, and `pointer_events` (press/release positions).
//! Widget `Response`s are never consulted for input.
//!
//! Protocol per frame:
//! 1. `begin()` snapshots input (call once, at the top of `update`).
//! 2. Layout code records hit rects (background → foreground order).
//! 3. Input is matched against LAST frame's rects is wrong — instead the
//!    caller processes rects recorded THIS frame at the END of layout,
//!    in reverse (topmost first), with click consumption.

use egui::{Pos2, Rect};
use std::time::Instant;

const CLICK_DRIFT: f32 = 12.0;
const DOUBLE_MS: u128 = 500;
const DOUBLE_DIST: f32 = 24.0;

#[derive(Default)]
pub struct ManState {
    pos: Option<Pos2>,
    down_l: bool,
    down_r: bool,
    prev_l: bool,
    prev_r: bool,
    rose_l: bool,
    rose_r: bool,
    org_l: Option<Pos2>,
    org_r: Option<Pos2>,
    rel_l: Option<Pos2>,
    rel_r: Option<Pos2>,
    consumed: bool,
    last_click_at: Option<Instant>,
    last_click_pos: Option<Pos2>,
    double_pending: bool,
    /// How many click/secondary matches fired (proves dispatch path).
    pub fires: u64,
    // key diagnostics (proves keyboard delivery for Phase 2 text fields)
    pub key_count: u64,
    pub text_count: u64,
    pub last_key: String,
}

impl ManState {
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.consumed = false;
        self.rel_l = None;
        self.rel_r = None;
        self.double_pending = false;
        ctx.input(|i| {
            self.pos = i.pointer.hover_pos();
            self.down_l = i.pointer.button_down(egui::PointerButton::Primary);
            self.down_r = i.pointer.button_down(egui::PointerButton::Secondary);
            if self.down_l || self.down_r {
                if let Some(o) = i.pointer.press_origin() {
                    if self.down_l {
                        self.org_l = Some(o);
                    }
                    if self.down_r {
                        self.org_r = Some(o);
                    }
                }
            }
            if self.down_l && !self.prev_l && self.org_l.is_none() {
                self.org_l = self.pos;
            }
            if self.down_r && !self.prev_r && self.org_r.is_none() {
                self.org_r = self.pos;
            }
            if !self.down_l && self.prev_l {
                self.rel_l = self.pos.or(self.org_l);
                self.org_l = None;
            }
            if !self.down_r && self.prev_r {
                self.rel_r = self.pos.or(self.org_r);
                self.org_r = None;
            }
            self.rose_l = self.down_l && !self.prev_l;
            self.rose_r = self.down_r && !self.prev_r;
            self.prev_l = self.down_l;
            self.prev_r = self.down_r;
            // keyboard delivery proof (Phase 2 text fields need this)
            for e in &i.events {
                match e {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => {
                        self.key_count += 1;
                        self.last_key = format!("{key:?}");
                    }
                    egui::Event::Text(_) => {
                        self.text_count += 1;
                    }
                    _ => {}
                }
            }
        });
    }

    pub fn pos(&self) -> Option<Pos2> {
        self.pos
    }

    pub fn down_left(&self) -> bool {
        self.down_l
    }

    #[allow(dead_code)]
    pub fn down(&self) -> bool {
        self.down_l || self.down_r
    }

    /// Hover with no buttons held.
    pub fn hover(&self, r: &Rect) -> bool {
        matches!(self.pos, Some(p) if r.contains(p)) && !self.down_l && !self.down_r
    }

    fn fire(&mut self, r: &Rect, rel: Option<Pos2>, org: Option<Pos2>) -> bool {
        if self.consumed {
            return false;
        }
        // Origin unknown (e.g. press arrived without position): fall back
        // to the release position so a clean release-inside still counts.
        let org = org.or(rel);
        match (rel, org) {
            (Some(q), Some(o))
                if r.contains(q) && r.contains(o) && q.distance(o) <= CLICK_DRIFT =>
            {
                self.consumed = true;
                self.fires += 1;
                true
            }
            _ => false,
        }
    }

    /// Left-click (press + release inside, minimal drift). Also arms
    /// double-click detection, read via `take_double` right after.
    pub fn clicked(&mut self, r: &Rect) -> bool {
        let (rel, org) = (self.rel_l, self.org_l);
        if !self.fire(r, rel, org) {
            return false;
        }
        let now = Instant::now();
        self.double_pending = matches!(
            (self.last_click_at, self.last_click_pos, rel),
            (Some(t), Some(p), Some(q))
                if now.duration_since(t).as_millis() <= DOUBLE_MS && p.distance(q) <= DOUBLE_DIST
        );
        self.last_click_at = Some(now);
        self.last_click_pos = rel;
        true
    }

    /// Consume the armed double-click (call right after `clicked`).
    pub fn take_double(&mut self) -> bool {
        let d = self.double_pending;
        self.double_pending = false;
        d
    }

    /// Right-click.
    pub fn secondary(&mut self, r: &Rect) -> bool {
        let (rel, org) = (self.rel_r, self.org_r);
        self.fire(r, rel, org)
    }

    /// Rising left-press edge inside `r` (for drag starts).
    /// Consumes, so only one drag starts per press.
    pub fn press_started_in(&mut self, r: &Rect) -> bool {
        if self.consumed {
            return false;
        }
        if self.rose_l && matches!(self.pos, Some(p) if r.contains(p)) {
            self.consumed = true;
            return true;
        }
        false
    }
}
