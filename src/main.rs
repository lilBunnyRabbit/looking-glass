#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{
    self, pos2, Align2, Color32, ColorImage, FontId, Rect, ResizeDirection, RichText, Sense,
    Stroke, TextureHandle, TextureOptions, Vec2, ViewportCommand, WindowLevel,
};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::RgbaImage;

const OPACITY_STEP: f32 = 0.1;
const MIN_OPACITY: f32 = 0.05;
const BRAND_DARKER: Color32 = Color32::from_rgb(0x0b, 0x15, 0x19);
const BRAND_BACKDROP: Color32 = Color32::from_rgba_unmultiplied_const(0x0b, 0x15, 0x19, 225);
const BRAND_BG: Color32 = Color32::from_rgb(0x17, 0x25, 0x2c);
const BRAND_BG_LIGHT: Color32 = Color32::from_rgb(0x1d, 0x2e, 0x36);
const BRAND_BG_LIGHTEST: Color32 = Color32::from_rgb(0x2a, 0x40, 0x4a);
const BRAND_FG: Color32 = Color32::from_rgb(0xb1, 0xab, 0xa8);
const BRAND_PRIMARY: Color32 = Color32::from_rgb(0xa4, 0xb4, 0x83);
const BRAND_PRIMARY_TINT: Color32 = Color32::from_rgb(0x2a, 0x37, 0x31);
const BRAND_COTTONTAIL: Color32 = Color32::from_rgb(0xe3, 0xf9, 0xb2);
const BRAND_WARNING: Color32 = Color32::from_rgb(0xe4, 0x9b, 0x39);
const BRAND_ERROR: Color32 = Color32::from_rgb(0xf8, 0x88, 0x76);
const LOCK_HINT_SECONDS: f64 = 3.0;
const HELP_STEPS: &[&str] = &[
    "1. Drop an image here, or double-click to open one",
    "2. Drag to move. Scroll or use the corner grip to resize",
    "3. Set the opacity with the slider",
    "4. Lock: clicks pass through to the app underneath",
];
const HELP_KEYS: &[(&str, &str)] = &[
    ("Ctrl+Alt+T", "lock / unlock"),
    ("Ctrl+Alt+Up / Down", "opacity"),
];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif"];

#[derive(Clone, Copy)]
enum Action {
    ToggleLock,
    OpacityUp,
    OpacityDown,
}

struct Hotkeys {
    _manager: GlobalHotKeyManager,
    rx: Receiver<Action>,
}

impl Hotkeys {
    fn register(ctx: &egui::Context) -> Option<Self> {
        let manager = GlobalHotKeyManager::new().ok()?;
        let mods = Some(Modifiers::CONTROL | Modifiers::ALT);
        let bindings = [
            (HotKey::new(mods, Code::KeyT), Action::ToggleLock),
            (HotKey::new(mods, Code::ArrowUp), Action::OpacityUp),
            (HotKey::new(mods, Code::ArrowDown), Action::OpacityDown),
        ];
        // Unlocking a click-through window is only possible via the hotkey, so it must register.
        manager.register(bindings[0].0).ok()?;
        for (hotkey, _) in &bindings[1..] {
            let _ = manager.register(*hotkey);
        }

        let ids: Vec<(u32, Action)> = bindings.iter().map(|(h, a)| (h.id(), *a)).collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                let _ = tx.send(*action);
                ctx.request_repaint();
            }
        }));

        Some(Self {
            _manager: manager,
            rx,
        })
    }
}

struct Picture {
    original: RgbaImage,
    texture: TextureHandle,
    size: Vec2,
}

struct App {
    picture: Option<Picture>,
    opacity: f32,
    flipped: bool,
    rotation: u8,
    grayscale: bool,
    on_top: bool,
    locked: bool,
    error: Option<String>,
    hotkeys: Option<Hotkeys>,
    pending: Option<PathBuf>,
    help: bool,
    lock_hint_until: f64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        apply_theme(&cc.egui_ctx);
        Self {
            picture: None,
            opacity: 0.5,
            flipped: false,
            rotation: 0,
            grayscale: false,
            on_top: true,
            locked: false,
            error: None,
            hotkeys: Hotkeys::register(&cc.egui_ctx),
            // The real max texture size is only known once the first frame has run.
            pending: initial,
            help: false,
            lock_hint_until: 0.0,
        }
    }

    fn open(&mut self, ctx: &egui::Context, path: &Path) {
        match image::open(path) {
            Ok(img) => {
                let original = fit_texture_limit(ctx, img.to_rgba8());
                self.flipped = false;
                self.rotation = 0;
                self.grayscale = false;
                let rendered = self.render(&original);
                let size = Vec2::new(rendered.width() as f32, rendered.height() as f32);
                let texture = upload(ctx, &rendered);
                self.picture = Some(Picture {
                    original,
                    texture,
                    size,
                });
                self.error = None;
                fit_window(ctx, size);
            }
            Err(err) => self.error = Some(format!("Can't open image: {err}")),
        }
    }

    fn render(&self, original: &RgbaImage) -> RgbaImage {
        let mut img = original.clone();
        if self.grayscale {
            for p in img.pixels_mut() {
                let [r, g, b, a] = p.0;
                let l = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;
                p.0 = [l, l, l, a];
            }
        }
        if self.flipped {
            img = image::imageops::flip_horizontal(&img);
        }
        match self.rotation {
            1 => image::imageops::rotate90(&img),
            2 => image::imageops::rotate180(&img),
            3 => image::imageops::rotate270(&img),
            _ => img,
        }
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let Some(picture) = &self.picture else { return };
        let rendered = self.render(&picture.original);
        let size = Vec2::new(rendered.width() as f32, rendered.height() as f32);
        let texture = upload(ctx, &rendered);
        if let Some(picture) = &mut self.picture {
            picture.texture = texture;
            picture.size = size;
        }
    }

    fn rotate(&mut self, ctx: &egui::Context) {
        self.rotation = (self.rotation + 1) % 4;
        self.refresh(ctx);
        if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                inner.height(),
                inner.width(),
            )));
        }
    }

    fn set_locked(&mut self, ctx: &egui::Context, locked: bool) {
        self.locked = locked;
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(locked));
        if locked {
            self.help = false;
            self.lock_hint_until = ctx.input(|i| i.time) + LOCK_HINT_SECONDS;
        }
        if locked && !self.on_top {
            self.set_on_top(ctx, true);
        }
    }

    fn set_on_top(&mut self, ctx: &egui::Context, on_top: bool) {
        self.on_top = on_top;
        let level = if on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::ToggleLock => self.set_locked(ctx, !self.locked),
            Action::OpacityUp => self.opacity = (self.opacity + OPACITY_STEP).min(1.0),
            Action::OpacityDown => self.opacity = (self.opacity - OPACITY_STEP).max(MIN_OPACITY),
        }
    }

    fn pick_file(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", IMAGE_EXTENSIONS)
            .pick_file()
        {
            self.open(ctx, &path);
        }
    }

    fn help_panel(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::unique("help"))
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, 18.0))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                bar_frame().inner_margin(16).show(ui, |ui| {
                    ui.heading("Looking Glass");
                    ui.add_space(6.0);
                    for step in HELP_STEPS {
                        ui.label(*step);
                    }
                    ui.add_space(8.0);
                    egui::Grid::new("help_keys")
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            for (key, what) in HELP_KEYS {
                                ui.monospace(*key);
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                    if self.hotkeys.is_none() {
                        ui.add_space(6.0);
                        ui.colored_label(
                            BRAND_WARNING,
                            "Ctrl+Alt+T is taken by another app, so Lock is off",
                        );
                    }
                    if self.picture.is_some() {
                        ui.add_space(8.0);
                        if ui.button("Got it").clicked() {
                            self.help = false;
                        }
                    }
                });
            });
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::unique("toolbar"))
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 8.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                bar_frame().show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 4.0);
                    ui.horizontal_wrapped(|ui| {
                        if icon(ui, Glyph::Open, "Open image", false).clicked() {
                            self.pick_file(ctx);
                        }
                        divider(ui);

                        ui.spacing_mut().slider_width = 90.0;
                        ui.add(
                            egui::Slider::new(&mut self.opacity, MIN_OPACITY..=1.0)
                                .show_value(false),
                        )
                        .on_hover_text("Opacity  (Ctrl+Alt+Up / Down)");
                        ui.add_sized(
                            [36.0, 28.0],
                            egui::Label::new(
                                RichText::new(format!("{:.0}%", self.opacity * 100.0))
                                    .monospace()
                                    .color(BRAND_FG),
                            ),
                        );
                        divider(ui);

                        let has_picture = self.picture.is_some();
                        ui.add_enabled_ui(has_picture, |ui| {
                            if icon(ui, Glyph::Flip, "Flip horizontally", self.flipped).clicked() {
                                self.flipped = !self.flipped;
                                self.refresh(ctx);
                            }
                            if icon(ui, Glyph::Rotate, "Rotate 90°", false).clicked() {
                                self.rotate(ctx);
                            }
                            if icon(ui, Glyph::Gray, "Grayscale", self.grayscale).clicked() {
                                self.grayscale = !self.grayscale;
                                self.refresh(ctx);
                            }
                            if icon(ui, Glyph::Fit, "Fit window to image", false).clicked() {
                                if let Some(p) = &self.picture {
                                    fit_window(ctx, p.size);
                                }
                            }
                        });
                        divider(ui);

                        if icon(ui, Glyph::Pin, "Always on top", self.on_top).clicked() {
                            self.set_on_top(ctx, !self.on_top);
                        }
                        let can_lock = self.hotkeys.is_some() && has_picture;
                        let lock = ui.add_enabled(
                            can_lock,
                            egui::Button::new(RichText::new("        Lock").color(BRAND_BG).strong())
                                .fill(BRAND_PRIMARY)
                                .min_size(Vec2::new(0.0, 28.0)),
                        );
                        paint_glyph(
                            ui.painter(),
                            pos2(lock.rect.left() + 14.0, lock.rect.center().y),
                            Glyph::Lock,
                            BRAND_BG,
                        );
                        let lock = if self.hotkeys.is_some() {
                            lock.on_hover_text("Clicks pass through. Ctrl+Alt+T to unlock")
                        } else {
                            lock.on_disabled_hover_text(
                                "Ctrl+Alt+T is taken by another app, so Lock is off",
                            )
                        };
                        if lock.clicked() {
                            self.set_locked(ctx, true);
                        }
                        divider(ui);

                        if icon(ui, Glyph::Help, "How it works", self.help).clicked() {
                            self.help = !self.help;
                        }
                        if icon(ui, Glyph::Close, "Close", false).clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    });
                    if let Some(err) = &self.error {
                        ui.colored_label(BRAND_ERROR, err);
                    }
                });
            });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(path) = self.pending.take() {
            self.open(ctx, &path);
        }

        let actions: Vec<Action> = self
            .hotkeys
            .as_ref()
            .map(|h| h.rx.try_iter().collect())
            .unwrap_or_default();
        for action in actions {
            self.apply(ctx, action);
        }

        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .find(|p| !p.as_os_str().is_empty())
        });
        if let Some(path) = dropped {
            self.open(ctx, &path);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let rect = ui.max_rect();
        let body = ui.interact(rect, ui.scope_id().with("body"), Sense::click_and_drag());
        if body.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if body.double_clicked() && self.picture.is_none() {
            self.pick_file(ctx);
        }

        let painter = ui.painter();
        match &self.picture {
            Some(picture) => {
                let tint = Color32::from_white_alpha((self.opacity * 255.0) as u8);
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(picture.texture.id(), fit_rect(rect, picture.size), uv, tint);
            }
            None => {
                painter.rect_filled(rect, 8.0, BRAND_BACKDROP);
            }
        }

        if self.locked {
            let now = ctx.input(|i| i.time);
            if now < self.lock_hint_until {
                let galley = painter.layout_no_wrap(
                    "Locked: Ctrl+Alt+T to unlock".into(),
                    FontId::proportional(18.0),
                    Color32::from_gray(235),
                );
                let text_rect = Rect::from_center_size(rect.center(), galley.size());
                painter.rect_filled(
                    text_rect.expand2(Vec2::new(14.0, 8.0)),
                    8.0,
                    BRAND_BACKDROP,
                );
                painter.galley(text_rect.min, galley, Color32::from_gray(235));
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    self.lock_hint_until - now,
                ));
            }
            return;
        }

        if self.picture.is_some() && ui.rect_contains_pointer(rect) {
            painter.rect_stroke(
                rect.shrink(0.5),
                0.0,
                Stroke::new(1.0, Color32::from_white_alpha(90)),
                egui::StrokeKind::Inside,
            );
        }

        let grip_rect = Rect::from_min_size(rect.max - Vec2::splat(18.0), Vec2::splat(18.0));
        let grip = ui.interact(grip_rect, ui.scope_id().with("grip"), Sense::drag());
        if grip.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(ResizeDirection::SouthEast));
        }
        let grip_color = if grip.hovered() {
            Color32::WHITE
        } else {
            Color32::from_white_alpha(140)
        };
        for offset in [4.0, 9.0, 14.0] {
            painter.line_segment(
                [
                    pos2(rect.max.x - offset, rect.max.y - 2.0),
                    pos2(rect.max.x - 2.0, rect.max.y - offset),
                ],
                Stroke::new(1.5, grip_color),
            );
        }

        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && ui.rect_contains_pointer(rect) {
            if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
                let scale = (1.0 + scroll * 0.002).clamp(0.5, 1.5);
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(
                    (inner.size() * scale).max(Vec2::splat(64.0)),
                ));
            }
        }

        if self.help || self.picture.is_none() {
            self.help_panel(ctx);
        }

        let hovering = ctx.input(|i| i.pointer.has_pointer());
        if !self.locked && (hovering || self.picture.is_none() || self.error.is_some()) {
            self.toolbar(ctx);
        }
    }
}

fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(BRAND_FG);
    v.panel_fill = BRAND_BG;
    v.window_fill = BRAND_BG;
    v.window_stroke = Stroke::new(1.0, BRAND_BG_LIGHTEST);
    v.window_corner_radius = 10.into();
    v.menu_corner_radius = 8.into();
    v.extreme_bg_color = BRAND_DARKER;
    v.slider_trailing_fill = true;
    v.selection.bg_fill = BRAND_PRIMARY_TINT;
    v.selection.stroke = Stroke::new(1.0, BRAND_PRIMARY);
    v.hyperlink_color = BRAND_PRIMARY;
    v.warn_fg_color = BRAND_WARNING;
    v.error_fg_color = BRAND_ERROR;

    let w = &mut v.widgets;
    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.corner_radius = 6.into();
        state.expansion = 0.0;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0, BRAND_FG);
    }
    w.noninteractive.bg_fill = BRAND_BG;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BRAND_BG_LIGHTEST);
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = BRAND_BG_LIGHTEST;
    w.hovered.weak_bg_fill = BRAND_BG_LIGHTEST;
    w.hovered.bg_fill = BRAND_BG_LIGHTEST;
    w.hovered.fg_stroke = Stroke::new(1.5, BRAND_COTTONTAIL);
    w.active.weak_bg_fill = BRAND_BG_LIGHT;
    w.active.bg_fill = BRAND_BG_LIGHTEST;
    w.active.fg_stroke = Stroke::new(1.5, BRAND_COTTONTAIL);
    ctx.set_visuals(v);
}

fn bar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BRAND_BG)
        .stroke(Stroke::new(1.0, BRAND_BG_LIGHTEST))
        .corner_radius(10)
        .inner_margin(6)
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(110),
        })
}

#[derive(Clone, Copy)]
enum Glyph {
    Open,
    Flip,
    Rotate,
    Gray,
    Fit,
    Pin,
    Lock,
    Help,
    Close,
}

fn icon(ui: &mut egui::Ui, glyph: Glyph, tip: &str, on: bool) -> egui::Response {
    let response = ui
        .add(egui::Button::selectable(on, "").min_size(Vec2::splat(28.0)))
        .on_hover_text(tip);
    let color = if !ui.is_enabled() {
        BRAND_FG.gamma_multiply(0.38)
    } else if response.hovered() {
        BRAND_COTTONTAIL
    } else if on {
        BRAND_PRIMARY
    } else {
        BRAND_FG
    };
    paint_glyph(ui.painter(), response.rect.center(), glyph, color);
    response
}

fn paint_glyph(painter: &egui::Painter, c: egui::Pos2, glyph: Glyph, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let p = |x: f32, y: f32| c + Vec2::new(x, y);
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<egui::Pos2> {
        (0..=16)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / 16.0).to_radians();
                p(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    };
    match glyph {
        Glyph::Open => {
            painter.add(egui::Shape::closed_line(
                vec![
                    p(-7.0, -5.0),
                    p(-2.5, -5.0),
                    p(-0.5, -3.0),
                    p(7.0, -3.0),
                    p(7.0, 5.5),
                    p(-7.0, 5.5),
                ],
                stroke,
            ));
        }
        Glyph::Flip => {
            painter.add(egui::Shape::closed_line(
                vec![p(-2.0, -5.0), p(-2.0, 5.0), p(-7.5, 5.0)],
                stroke,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![p(2.0, -5.0), p(7.5, 5.0), p(2.0, 5.0)],
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::dashed_line(
                &[p(0.0, -7.5), p(0.0, 7.5)],
                Stroke::new(1.0, color),
                2.0,
                2.0,
            ));
        }
        Glyph::Rotate => {
            let points = arc(0.0, 0.0, 5.5, -60.0, 225.0);
            let end = points[0];
            painter.add(egui::Shape::line(points, stroke));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    end + Vec2::new(-3.5, -1.5),
                    end + Vec2::new(1.5, -3.0),
                    end + Vec2::new(0.5, 2.5),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Gray => {
            painter.circle_stroke(c, 6.0, stroke);
            painter.add(egui::Shape::convex_polygon(
                arc(0.0, 0.0, 6.0, -90.0, 90.0),
                color,
                Stroke::NONE,
            ));
        }
        Glyph::Fit => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                painter.add(egui::Shape::line(
                    vec![
                        p(sx * 6.5, sy * 2.5),
                        p(sx * 6.5, sy * 6.0),
                        p(sx * 3.0, sy * 6.0),
                    ],
                    stroke,
                ));
            }
        }
        Glyph::Pin => {
            painter.add(egui::Shape::closed_line(
                vec![
                    p(-3.0, -7.0),
                    p(3.0, -7.0),
                    p(2.0, -2.0),
                    p(5.0, 1.5),
                    p(-5.0, 1.5),
                    p(-2.0, -2.0),
                ],
                stroke,
            ));
            painter.line_segment([p(0.0, 1.5), p(0.0, 7.5)], stroke);
        }
        Glyph::Lock => {
            painter.rect_stroke(
                Rect::from_min_max(p(-5.5, -1.0), p(5.5, 6.5)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(arc(0.0, -3.0, 3.5, 180.0, 360.0), stroke));
            painter.line_segment([p(-3.5, -3.0), p(-3.5, -1.0)], stroke);
            painter.line_segment([p(3.5, -3.0), p(3.5, -1.0)], stroke);
        }
        Glyph::Help => {
            painter.text(
                c,
                Align2::CENTER_CENTER,
                "?",
                FontId::proportional(16.0),
                color,
            );
        }
        Glyph::Close => {
            painter.line_segment([p(-4.5, -4.5), p(4.5, 4.5)], stroke);
            painter.line_segment([p(4.5, -4.5), p(-4.5, 4.5)], stroke);
        }
    }
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 28.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(6.0),
        Stroke::new(1.0, BRAND_BG_LIGHTEST),
    );
}

fn fit_texture_limit(ctx: &egui::Context, img: RgbaImage) -> RgbaImage {
    let max = ctx.input(|i| i.max_texture_side) as u32;
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return img;
    }
    let scale = max as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * scale) as u32).clamp(1, max),
        ((h as f32 * scale) as u32).clamp(1, max),
    );
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
}

fn upload(ctx: &egui::Context, img: &RgbaImage) -> TextureHandle {
    let color = ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    ctx.load_texture("picture", color, TextureOptions::LINEAR)
}

fn fit_rect(area: Rect, size: Vec2) -> Rect {
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

fn fit_window(ctx: &egui::Context, size: Vec2) {
    let max = ctx
        .input(|i| i.viewport().monitor_size)
        .unwrap_or(Vec2::new(1600.0, 900.0))
        * 0.8;
    let scale = (max.x / size.x).min(max.y / size.y).min(1.0);
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size * scale));
}

fn main() -> eframe::Result {
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Looking Glass")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("bundled icon"),
            )
            .with_inner_size([520.0, 360.0])
            .with_min_inner_size([64.0, 64.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Looking Glass",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, initial)))),
    )
}
