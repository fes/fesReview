use crate::backend::{Backend, Event};
#[cfg(target_os = "macos")]
use crate::controller::{self, Action as ControllerAction};
use base64::Engine;
use eframe::egui::{self, Color32, Key};
use fesreview::model::{timecode, FrameTime, NoteState, Session};
use serde_json::json;
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
enum Direction {
    Backward,
    Forward,
}

impl Direction {
    fn delta(self) -> i64 {
        match self {
            Self::Backward => -1,
            Self::Forward => 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ReviewAction {
    TogglePlay,
    Boundary(Direction),
    JumpSecond(Direction),
    StepFrame(Direction),
    Screenshot,
    ToggleDictation,
}

#[cfg(target_os = "macos")]
impl From<ControllerAction> for ReviewAction {
    fn from(action: ControllerAction) -> Self {
        match action {
            ControllerAction::TogglePlay => Self::TogglePlay,
            ControllerAction::SeekStart => Self::Boundary(Direction::Backward),
            ControllerAction::SeekEnd => Self::Boundary(Direction::Forward),
            ControllerAction::BackOneSecond => Self::JumpSecond(Direction::Backward),
            ControllerAction::ForwardOneSecond => Self::JumpSecond(Direction::Forward),
            ControllerAction::Screenshot => Self::Screenshot,
            ControllerAction::ToggleDictation => Self::ToggleDictation,
            ControllerAction::PreviousFrame => Self::StepFrame(Direction::Backward),
            ControllerAction::NextFrame => Self::StepFrame(Direction::Forward),
        }
    }
}

pub struct ReviewApp {
    backend: Option<Backend>,
    session: Option<Session>,
    opening: Option<Receiver<Result<Session, String>>>,
    texture: Option<egui::TextureHandle>,
    anchor: FrameTime,
    duration: f64,
    frames: u64,
    playing: bool,
    ready: bool,
    loading: bool,
    show_notes: bool,
    text_focus: bool,
    active: Option<Uuid>,
    starting: bool,
    stop_requested: bool,
    stopping: bool,
    error: Option<String>,
    dirty: bool,
    last_save: Instant,
    last_poll: Instant,
    poll_pending: bool,
    closing: bool,
    scrub: Option<f64>,
    #[cfg(target_os = "macos")]
    controller: controller::Bridge,
    #[cfg(target_os = "macos")]
    controller_focus: Option<controller::Focus>,
    #[cfg(test)]
    commands: Vec<serde_json::Value>,
    #[cfg(test)]
    events: std::collections::VecDeque<Event>,
}
impl ReviewApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        #[cfg(target_os = "macos")] shutdown: controller::ShutdownSender,
    ) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(0x0e, 0x13, 0x19);
        visuals.extreme_bg_color = Color32::from_rgb(0x11, 0x16, 0x1e);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x1a, 0x22, 0x2c);
        visuals.selection.bg_fill = Color32::from_rgb(0x28, 0x51, 0x6b);
        visuals.selection.stroke.color = Color32::from_rgb(0x42, 0xbf, 0xd0);
        cc.egui_ctx.set_visuals(visuals);
        let (backend, error) = match Backend::start(cc.egui_ctx.clone()) {
            Ok(b) => (Some(b), None),
            Err(e) => (None, Some(e)),
        };
        #[cfg(target_os = "macos")]
        let controller = controller::Bridge::new(Some(shutdown));
        #[cfg(target_os = "macos")]
        let controller_focus = controller.observe_focus();
        Self {
            backend,
            session: None,
            opening: None,
            texture: None,
            anchor: FrameTime::default(),
            duration: 0.0,
            frames: 0,
            playing: false,
            ready: false,
            loading: false,
            show_notes: false,
            text_focus: false,
            active: None,
            starting: false,
            stop_requested: false,
            stopping: false,
            error,
            dirty: false,
            last_save: Instant::now(),
            last_poll: Instant::now(),
            poll_pending: false,
            closing: false,
            scrub: None,
            #[cfg(target_os = "macos")]
            controller,
            #[cfg(target_os = "macos")]
            controller_focus,
            #[cfg(test)]
            commands: Vec::new(),
            #[cfg(test)]
            events: std::collections::VecDeque::new(),
        }
    }
    fn send(&mut self, command: serde_json::Value) {
        #[cfg(test)]
        self.commands.push(command.clone());
        if let Some(b) = &self.backend {
            if let Err(e) = b.send(command) {
                self.error = Some(e);
            }
        }
    }
    fn save(&mut self) {
        if let Some(s) = &self.session {
            match s.save() {
                Ok(()) => self.dirty = false,
                Err(e) => {
                    self.dirty = true;
                    self.error = Some(format!("Could not save notes: {e}"));
                }
            }
        }
        self.last_save = Instant::now();
    }
    fn open(&mut self, ctx: &egui::Context) {
        if self.active.is_some()
            || self.loading
            || self.backend.is_none()
            || self.session.as_ref().is_some_and(|s| {
                s.review
                    .notes
                    .iter()
                    .any(|n| n.state == NoteState::Transcribing)
            })
        {
            return;
        }
        self.save();
        if self.dirty {
            return;
        }
        #[cfg(target_os = "macos")]
        self.controller.suspend();
        self.send(json!({"command":"pause"}));
        self.playing = false;
        let Some(video) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "m4v"])
            .pick_file()
        else {
            return;
        };
        // Keep one review next to each complete filename, or choose a writable destination.
        let default = video.with_file_name(format!(
            "{}.fesreview",
            video.file_name().unwrap_or_default().to_string_lossy()
        ));
        let directory = if default.exists() {
            default
        } else {
            let Some(parent) = rfd::FileDialog::new()
                .set_title("Choose where to save the review")
                .set_directory(video.parent().unwrap_or(std::path::Path::new(".")))
                .pick_folder()
            else {
                return;
            };
            parent.join(default.file_name().unwrap_or_default())
        };
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.directory == directory)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.opening = Some(rx);
        self.loading = true;
        self.ready = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Session::open(&video, directory).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
    }
    fn start_note(&mut self) {
        if !self.ready || self.active.is_some() || self.anchor.timescale <= 0 || self.dirty {
            return;
        }
        let Some(s) = self.session.as_mut() else {
            return;
        };
        match s.begin_note(self.anchor) {
            Ok(id) => {
                let audio = s.directory.join(format!("audio/{id}.wav"));
                self.active = Some(id);
                self.starting = true;
                self.stop_requested = false;
                self.playing = false;
                self.show_notes = true;
                self.send(json!({"command":"record","id":id,"frame_index":self.anchor.frame_index,"audio":audio}));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    fn stop_note(&mut self) {
        if self.active.is_none() || self.stopping {
            return;
        }
        if self.starting {
            self.stop_requested = true;
            return;
        }
        self.stopping = true;
        self.send(json!({"command":"stop_record"}));
    }
    fn drain(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.opening.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.opening = None;
            match result {
                Ok(session) => {
                    let path = session.review.source.path.clone();
                    self.session = Some(session);
                    self.texture = None;
                    self.ready = false;
                    self.anchor = FrameTime::default();
                    self.duration = 0.0;
                    self.frames = 0;
                    self.poll_pending = false;
                    self.send(json!({"command":"open","path":path}));
                }
                Err(e) => {
                    self.error = Some(e);
                    self.loading = false;
                    self.ready = self.frames > 0;
                }
            }
        }
        while let Some(event) = self.next_event() {
            match event {
                Event::Opened { duration, frames } => {
                    self.duration = duration;
                    self.frames = frames;
                    self.ready = true;
                    self.loading = false;
                }
                Event::Frame {
                    anchor,
                    png,
                    playing,
                } => {
                    self.poll_pending = false;
                    if !png.is_empty() {
                        match base64::engine::general_purpose::STANDARD
                            .decode(png)
                            .map_err(|e| e.to_string())
                            .and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string()))
                        {
                            Ok(image) => {
                                let rgba = image.to_rgba8();
                                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                                    [rgba.width() as usize, rgba.height() as usize],
                                    rgba.as_raw(),
                                );
                                if let Some(t) = self.texture.as_mut() {
                                    t.set(pixels, egui::TextureOptions::LINEAR);
                                } else {
                                    self.texture = Some(ctx.load_texture(
                                        "video",
                                        pixels,
                                        egui::TextureOptions::LINEAR,
                                    ));
                                }
                                self.anchor = anchor;
                            }
                            Err(e) => self.error = Some(format!("Video frame: {e}")),
                        }
                    }
                    self.playing = playing;
                }
                Event::Recording { id, anchor } => {
                    if self.active == Some(id) {
                        self.starting = false;
                    }
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        note.anchor = anchor;
                        note.timestamp_seconds = anchor.seconds();
                    }
                    self.dirty = true;
                    self.save();
                    if self.stop_requested || self.dirty || self.closing {
                        self.stop_note();
                    }
                }
                Event::Recorded { id } => {
                    if self.active == Some(id) {
                        self.active = None;
                        self.stopping = false;
                        self.starting = false;
                    }
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        note.state = NoteState::Transcribing;
                    }
                    self.dirty = true;
                    self.save();
                }
                Event::LiveTranscript { id, text } => {
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        if !note.edited && note.state == NoteState::Recording {
                            note.transcript = text;
                        }
                    }
                }
                Event::Screenshot { id } => {
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        note.screenshot = Some(format!("frames/{id}.png"));
                    }
                    self.dirty = true;
                    self.save();
                }
                Event::Transcript { id, text } => {
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        if !note.edited {
                            note.transcript = text;
                        }
                        note.state = NoteState::Ready;
                        note.error = None;
                    }
                    self.dirty = true;
                    self.save();
                }
                Event::TranscriptionStatus { id, message } => {
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        note.error = Some(message);
                    }
                }
                Event::Failed {
                    id,
                    operation,
                    message,
                } => {
                    self.poll_pending = false;
                    if let (Some(id), "screenshot") = (id, operation.as_str()) {
                        if let Some(s) = self.session.as_mut() {
                            s.review.notes.retain(|n| {
                                n.id != id || !n.audio.is_empty() || n.screenshot.is_some()
                            });
                        }
                        self.dirty = true;
                        self.save();
                    } else if let Some(id) = id {
                        if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                            note.state = NoteState::Failed;
                            note.error = Some(message.clone());
                        }
                        if self.active == Some(id) {
                            self.active = None;
                            self.starting = false;
                            self.stopping = false;
                        }
                        self.dirty = true;
                        self.save();
                    }
                    if operation == "open" || operation == "service" {
                        self.loading = false;
                        self.ready = false;
                        self.active = None;
                        self.starting = false;
                        self.stopping = false;
                    }
                    self.error = Some(message);
                }
            }
        }
    }
    fn next_event(&mut self) -> Option<Event> {
        #[cfg(test)]
        if let Some(event) = self.events.pop_front() {
            return Some(event);
        }
        self.backend.as_ref().and_then(|b| b.events.try_recv().ok())
    }
    fn dispatch(&mut self, action: ReviewAction) {
        match action {
            ReviewAction::Screenshot => self.screenshot(),
            ReviewAction::ToggleDictation => {
                if self.active.is_some() {
                    self.stop_note();
                } else {
                    self.start_note();
                }
            }
            _ if !self.ready || self.active.is_some() => {}
            ReviewAction::TogglePlay => {
                self.send(json!({"command":if self.playing{"pause"}else{"play"}}));
                self.playing = !self.playing;
            }
            ReviewAction::Boundary(direction) if self.frames > 0 => {
                let frame = match direction {
                    Direction::Backward => 0,
                    Direction::Forward => self.frames - 1,
                };
                self.send(json!({"command":"frame","frame_index":frame}));
                self.playing = false;
            }
            ReviewAction::Boundary(_) => {}
            ReviewAction::JumpSecond(direction) => {
                self.send(json!({"command":"jump","seconds":direction.delta() as f64}));
                self.playing = false;
            }
            ReviewAction::StepFrame(direction) => {
                self.send(json!({"command":"step","delta":direction.delta()}));
                self.playing = false;
            }
        }
    }
    // Attach to the note being dictated, else to a note already on this frame, else start a screenshot-only note.
    fn screenshot(&mut self) {
        if !self.ready || self.loading || self.stopping || self.anchor.timescale <= 0 {
            return;
        }
        let frame = self.anchor.frame_index;
        let anchor = self.anchor;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let existing = self.active.or_else(|| {
            s.review
                .notes
                .iter()
                .rev()
                .find(|n| n.anchor.frame_index == frame && n.screenshot.is_none())
                .map(|n| n.id)
        });
        let id = match existing {
            Some(id) => id,
            None => match s.begin_screenshot_note(anchor) {
                Ok(id) => id,
                Err(e) => {
                    self.error = Some(e.to_string());
                    return;
                }
            },
        };
        let path = s.directory.join(format!("frames/{id}.png"));
        self.show_notes = true;
        self.send(json!({"command":"screenshot","id":id,"frame_index":frame,"path":path}));
    }

    #[cfg(target_os = "macos")]
    fn controller_input(&mut self, close: bool, foreground: impl Fn(&Self) -> Option<u64>) {
        let epoch = if self.controller_ready(close) {
            foreground(self)
        } else {
            None
        };
        if let Some(action) = self.controller.next(epoch) {
            if foreground(self) == epoch {
                self.dispatch(action.into());
                if !self.controller_ready(close) {
                    self.controller.suspend();
                }
            } else {
                self.controller.suspend();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn controller_ready(&self, close: bool) -> bool {
        !close
            && !self.closing
            && self.ready
            && !self.loading
            && self.opening.is_none()
            && self.session.is_some()
            && !self.starting
            && !self.stopping
            && !self.text_focus
            && self.scrub.is_none()
    }
}
impl eframe::App for ReviewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain(ctx);
        let close = ctx.input(|i| i.viewport().close_requested());
        if close {
            #[cfg(target_os = "macos")]
            self.controller.suspend();
            self.save();
            if self.active.is_some() || self.dirty {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.closing = true;
                self.stop_note();
            }
        }
        if self.closing && self.active.is_none() && !self.dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Only a note editor should keep keyboard focus; otherwise a clicked button or slider
        // swallows Space/arrows.
        if !self.text_focus {
            if let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
            let (space, left, right, home, end, r, s, jump) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Space),
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::Home),
                    i.key_pressed(Key::End),
                    i.key_pressed(Key::R),
                    i.key_pressed(Key::S) && !i.modifiers.command,
                    i.modifiers.shift || i.modifiers.ctrl,
                )
            });
            // Hide the keys from egui's focus navigation and from the slider.
            ctx.input_mut(|i| {
                i.events.retain(|e| {
                    !matches!(e, egui::Event::Key { key, .. } if matches!(key, Key::Space | Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End | Key::R | Key::S))
                })
            });
            if space {
                self.dispatch(ReviewAction::TogglePlay);
            }
            for (pressed, dir) in [(left, Direction::Backward), (right, Direction::Forward)] {
                if pressed {
                    if jump {
                        self.dispatch(ReviewAction::JumpSecond(dir));
                    } else {
                        self.dispatch(ReviewAction::StepFrame(dir));
                    }
                }
            }
            for (pressed, dir) in [(home, Direction::Backward), (end, Direction::Forward)] {
                if pressed {
                    self.dispatch(ReviewAction::Boundary(dir));
                }
            }
            if r {
                self.dispatch(ReviewAction::ToggleDictation);
            }
            if s {
                self.dispatch(ReviewAction::Screenshot);
            }
        }
        self.text_focus = false;
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::O)) {
            self.open(ctx);
        }
        egui::TopBottomPanel::top("menu").show(ctx,|ui|{
            ui.horizontal(|ui|{
                ui.add_enabled_ui(self.active.is_none() && !self.loading && !self.session.as_ref().is_some_and(|s|s.review.notes.iter().any(|n|n.state==NoteState::Transcribing)),|ui|{
                    if ui.button("＋").on_hover_text("Open video · ⌘O").clicked(){self.open(ctx);}
                });
                if let Some(s)=&self.session{ui.weak(s.review.source.path.file_name().unwrap_or_default().to_string_lossy());}
                if self.loading{ui.spinner();}
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                    ui.menu_button("⋯",|ui|{
                        if ui.button("Show review folder").clicked(){if let Some(s)=&self.session{let _=std::process::Command::new("open").arg(&s.directory).spawn();}}
                        #[cfg(target_os = "macos")]
                        self.controller.menu(ui);
                        ui.separator();ui.weak("Space: play / pause\nHome / End: first / last frame\n← →: previous / next frame\n⇧← ⇧→ (or ⌃): back / forward 1 s\nR: start / stop dictation\nS: screenshot of this frame");
                    });
                    ui.toggle_value(&mut self.show_notes,"☰").on_hover_text("Notes");
                });
            });
        });
        if let Some(message) = self.error.clone() {
            egui::TopBottomPanel::top("error").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(Color32::from_rgb(0xd9, 0x68, 0x68), message);
                    if self.dirty && ui.button("Retry save").clicked() {
                        self.save();
                    }
                    #[cfg(target_os = "macos")]
                    if let Some(message) = self.controller.error() {
                        egui::TopBottomPanel::top("controller_error").show(ctx, |ui| {
                            ui.colored_label(Color32::from_rgb(0xd9, 0x68, 0x68), message);
                        });
                    }
                    if ui.small_button("×").clicked() {
                        self.error = None;
                    }
                });
            });
        }
        egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.ready && self.active.is_none(), |ui| {
                    if ui.button("⏮").on_hover_text("First frame · Home").clicked() {
                        self.dispatch(ReviewAction::Boundary(Direction::Backward));
                    }
                    if ui
                        .button("-1s")
                        .on_hover_text("Back 1 second · ⇧←")
                        .clicked()
                    {
                        self.dispatch(ReviewAction::JumpSecond(Direction::Backward));
                    }
                    if ui
                        .button("-1f")
                        .on_hover_text("Previous frame · ←")
                        .clicked()
                    {
                        self.dispatch(ReviewAction::StepFrame(Direction::Backward));
                    }
                    if ui
                        .button(if self.playing { "⏸" } else { "▶" })
                        .on_hover_text("Play / pause · Space")
                        .clicked()
                    {
                        self.dispatch(ReviewAction::TogglePlay);
                    }
                    if ui.button("+1f").on_hover_text("Next frame · →").clicked() {
                        self.dispatch(ReviewAction::StepFrame(Direction::Forward));
                    }
                    if ui
                        .button("+1s")
                        .on_hover_text("Forward 1 second · ⇧→")
                        .clicked()
                    {
                        self.dispatch(ReviewAction::JumpSecond(Direction::Forward));
                    }
                    if ui.button("⏭").on_hover_text("Last frame · End").clicked() {
                        self.dispatch(ReviewAction::Boundary(Direction::Forward));
                    }
                });
                ui.monospace(timecode(self.anchor.seconds()));
                ui.add_enabled_ui(self.ready && self.active.is_none(), |ui| {
                    let mut seconds = self.scrub.unwrap_or(self.anchor.seconds());
                    let width = (ui.available_width() - 360.0).max(80.0);
                    ui.spacing_mut().slider_width = width;
                    let slider = ui.add(
                        egui::Slider::new(&mut seconds, 0.0..=self.duration.max(0.01))
                            .show_value(false),
                    );
                    if slider.dragged() {
                        self.scrub = Some(seconds);
                    }
                    if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                        self.scrub = None;
                        self.send(json!({"command":"seek","seconds":seconds}));
                        self.playing = false;
                    }
                });
                ui.add_enabled_ui(
                    self.ready && !self.stopping && (!self.dirty || self.active.is_some()),
                    |ui| {
                        let text = if self.active.is_some() {
                            egui::RichText::new("■ Stop").color(Color32::RED)
                        } else {
                            egui::RichText::new("● Dictate")
                        };
                        if ui
                            .button(text)
                            .on_hover_text("Start / stop dictation · R")
                            .clicked()
                        {
                            self.dispatch(ReviewAction::ToggleDictation);
                        }
                    },
                );
                ui.add_enabled_ui(self.ready && !self.loading && !self.stopping, |ui| {
                    if ui
                        .button("Screenshot")
                        .on_hover_text("Save this frame with a note · S")
                        .clicked()
                    {
                        self.dispatch(ReviewAction::Screenshot);
                    }
                });
                if self.active.is_some() {
                    if self.starting {
                        ui.spinner();
                    } else {
                        ui.colored_label(Color32::RED, "Recording");
                    }
                } else if self.frames > 0 {
                    ui.weak(format!("#{}", self.anchor.frame_index));
                }
            });
        });
        if self.show_notes {
            egui::SidePanel::right("notes")
                .default_width(290.0)
                .width_range(220.0..=450.0)
                .show(ctx, |ui| {
                    if self.loading {
                        ui.disable();
                    }
                    let mut seek = None;
                    let mut retry = None;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if let Some(s) = self.session.as_mut() {
                            for note in &mut s.review.notes {
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.active.is_none(),
                                            egui::Button::new(timecode(note.timestamp_seconds)),
                                        )
                                        .clicked()
                                    {
                                        seek = Some(note.anchor.frame_index);
                                    }
                                    if matches!(
                                        note.state,
                                        NoteState::Transcribing | NoteState::Recording
                                    ) {
                                        ui.spinner();
                                    }
                                    if note.screenshot.is_some() {
                                        ui.weak("▣").on_hover_text("Screenshot saved");
                                    }
                                    if !note.audio.is_empty()
                                        && matches!(
                                            note.state,
                                            NoteState::Failed | NoteState::Interrupted
                                        )
                                        && ui
                                            .small_button("↻")
                                            .on_hover_text("Retry transcription")
                                            .clicked()
                                    {
                                        retry = Some((note.id, s.directory.join(&note.audio)));
                                        note.state = NoteState::Transcribing;
                                        note.error = None;
                                        self.dirty = true;
                                    }
                                });
                                let editor = ui.add(
                                    egui::TextEdit::multiline(&mut note.transcript)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(3)
                                        .hint_text("Note"),
                                );
                                self.text_focus |= editor.has_focus();
                                if editor.changed() {
                                    note.edited = true;
                                    self.dirty = true;
                                }
                                if let Some(error) = &note.error {
                                    ui.weak(error);
                                }
                                ui.separator();
                            }
                            if s.review.notes.is_empty() {
                                ui.weak("No notes yet");
                            }
                        }
                    });
                    if let Some(index) = seek {
                        self.send(json!({"command":"frame","frame_index":index}));
                        self.playing = false;
                    }
                    if let Some((id, path)) = retry {
                        self.save();
                        self.send(json!({"command":"transcribe","id":id,"audio":path}));
                    }
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(texture) = &self.texture {
                ui.centered_and_justified(|ui| {
                    ui.add(
                        egui::Image::new(texture).fit_to_exact_size(
                            texture.size_vec2()
                                * (ui.available_width() / texture.size_vec2().x)
                                    .min(ui.available_height() / texture.size_vec2().y),
                        ),
                    );
                });
            } else {
                ui.centered_and_justified(|ui| {
                    if ui
                        .add_enabled(
                            self.backend.is_some() && !self.loading,
                            egui::Button::new("Open video"),
                        )
                        .clicked()
                    {
                        self.open(ctx);
                    }
                });
            }
        });
        if self.dirty && self.last_save.elapsed() > Duration::from_millis(500) {
            self.save();
        }
        #[cfg(target_os = "macos")]
        self.controller_input(close, |app| {
            ctx.input(|i| i.focused)
                .then(|| app.controller_focus.as_ref()?.foreground(_frame))
                .flatten()
        });
        if self.ready && !self.poll_pending && self.last_poll.elapsed() > Duration::from_millis(33)
        {
            self.send(json!({"command":"poll"}));
            self.poll_pending = true;
            self.last_poll = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> ReviewApp {
        ReviewApp {
            backend: None,
            session: None,
            opening: None,
            texture: None,
            anchor: FrameTime {
                value: 3,
                timescale: 10,
                frame_index: 2,
            },
            duration: 1.0,
            frames: 5,
            playing: false,
            ready: true,
            loading: false,
            show_notes: false,
            text_focus: false,
            active: None,
            starting: false,
            stop_requested: false,
            stopping: false,
            error: None,
            dirty: false,
            last_save: Instant::now(),
            last_poll: Instant::now(),
            poll_pending: false,
            closing: false,
            scrub: None,
            #[cfg(target_os = "macos")]
            controller: controller::Bridge::new(None),
            #[cfg(target_os = "macos")]
            controller_focus: None,
            commands: Vec::new(),
            events: Default::default(),
        }
    }

    #[test]
    fn transport_dispatches_exact_indices_and_each_detent() {
        let mut app = app();
        app.playing = true;
        app.dispatch(ReviewAction::Boundary(Direction::Forward));
        app.dispatch(ReviewAction::Boundary(Direction::Backward));
        app.dispatch(ReviewAction::StepFrame(Direction::Forward));
        app.dispatch(ReviewAction::StepFrame(Direction::Forward));
        app.dispatch(ReviewAction::StepFrame(Direction::Backward));
        app.dispatch(ReviewAction::JumpSecond(Direction::Backward));
        app.dispatch(ReviewAction::JumpSecond(Direction::Forward));
        assert_eq!(
            app.commands,
            vec![
                json!({"command":"frame","frame_index":4}),
                json!({"command":"frame","frame_index":0}),
                json!({"command":"step","delta":1}),
                json!({"command":"step","delta":1}),
                json!({"command":"step","delta":-1}),
                json!({"command":"jump","seconds":-1.0}),
                json!({"command":"jump","seconds":1.0}),
            ]
        );
        assert!(!app.playing);
        app.dispatch(ReviewAction::TogglePlay);
        app.dispatch(ReviewAction::TogglePlay);
        assert_eq!(
            &app.commands[7..],
            &[json!({"command":"play"}), json!({"command":"pause"})]
        );
        app.ready = false;
        app.dispatch(ReviewAction::StepFrame(Direction::Forward));
        app.ready = true;
        app.frames = 0;
        app.dispatch(ReviewAction::Boundary(Direction::Forward));
        app.active = Some(Uuid::new_v4());
        app.dispatch(ReviewAction::TogglePlay);
        app.dispatch(ReviewAction::StepFrame(Direction::Backward));
        assert_eq!(app.commands.len(), 9);
    }

    #[test]
    fn dictation_and_screenshot_share_visible_anchor_and_recording_lifecycle() {
        let dir = tempfile::Builder::new()
            .prefix("action-test-")
            .tempdir_in(".")
            .unwrap();
        let video = dir.path().join("video.mov");
        std::fs::write(&video, b"test video").unwrap();
        let mut app = app();
        app.session = Some(Session::open(&video, dir.path().join("review")).unwrap());
        app.playing = true;
        app.dispatch(ReviewAction::ToggleDictation);
        let id = app.active.unwrap();
        assert!(app.starting && !app.playing && app.show_notes);
        assert_eq!(app.commands[0]["command"], "record");
        assert_eq!(app.commands[0]["frame_index"], 2);
        app.dispatch(ReviewAction::Screenshot);
        assert_eq!(app.commands[1]["command"], "screenshot");
        assert_eq!(app.commands[1]["frame_index"], 2);
        assert_eq!(app.commands[1]["id"], id.to_string());
        app.dispatch(ReviewAction::ToggleDictation);
        assert!(app.stop_requested);
        assert_eq!(app.commands.len(), 2);
        app.events.push_back(Event::Recording {
            id,
            anchor: app.anchor,
        });
        app.drain(&egui::Context::default());
        assert!(app.stopping && !app.starting);
        assert_eq!(app.commands[2], json!({"command":"stop_record"}));
        app.dispatch(ReviewAction::ToggleDictation);
        app.dispatch(ReviewAction::Screenshot);
        assert_eq!(app.commands.len(), 3);
        app.events.push_back(Event::Recorded { id });
        app.drain(&egui::Context::default());
        assert!(app.active.is_none() && !app.stopping);
        assert_eq!(
            app.session.as_ref().unwrap().review.notes[0].state,
            NoteState::Transcribing
        );
        app.dirty = true;
        app.dispatch(ReviewAction::ToggleDictation);
        assert!(app.active.is_none());
    }

    #[test]
    fn screenshot_uses_displayed_frame_even_after_navigation_command() {
        let dir = tempfile::Builder::new()
            .prefix("action-test-")
            .tempdir_in(".")
            .unwrap();
        let video = dir.path().join("video.mov");
        std::fs::write(&video, b"test video").unwrap();
        let mut app = app();
        app.session = Some(Session::open(&video, dir.path().join("review")).unwrap());
        app.dispatch(ReviewAction::StepFrame(Direction::Forward));
        app.dispatch(ReviewAction::Screenshot);
        assert_eq!(app.commands[1]["frame_index"], 2);
        assert_eq!(
            app.session.as_ref().unwrap().review.notes[0].anchor,
            app.anchor
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn controller_dictation_reaches_dispatch_only_in_the_current_safe_window() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let video = dir.path().join("video.mov");
        std::fs::write(&video, b"test video").unwrap();
        let mut app = app();
        app.session = Some(Session::open(&video, dir.path().join("review")).unwrap());
        let queue = || controller::tests::queued(7, Instant::now());
        app.controller = queue();
        app.controller_input(false, |_| None);
        app.controller_input(false, |_| Some(7));
        app.controller = queue();
        app.controller_input(false, |_| Some(8));
        app.controller = queue();
        app.controller_input(true, |_| Some(7));
        app.controller = queue();
        app.loading = true;
        app.controller_input(false, |_| Some(7));
        app.loading = false;
        app.controller_input(false, |_| Some(7));
        assert!(app.commands.is_empty());
        app.controller = queue();
        app.controller_input(false, |_| Some(7));
        assert_eq!(app.commands[0]["command"], "record");
        app.controller = queue();
        app.controller_input(false, |_| Some(7)); // Recording is still starting.
        assert!(!app.stop_requested);
        app.events.push_back(Event::Recording {
            id: app.active.unwrap(),
            anchor: app.anchor,
        });
        app.drain(&egui::Context::default());
        app.controller_input(false, |_| Some(7)); // Drop the transition's queue.
        assert_eq!(app.commands.len(), 1);
        app.controller = queue();
        app.controller_input(false, |_| Some(7));
        assert_eq!(app.commands[1]["command"], "stop_record");
    }
}
