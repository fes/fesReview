use crate::backend::{Backend, Event};
use base64::Engine;
use eframe::egui::{self, Color32, Key};
use fesreview::model::{timecode, FrameTime, NoteState, Session};
use serde_json::json;
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use uuid::Uuid;

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
    show_notes: bool,
    snapshots: bool,
    resume: bool,
    active: Option<Uuid>,
    starting: bool,
    stop_requested: bool,
    stopping: bool,
    held_record: bool,
    error: Option<String>,
    dirty: bool,
    last_save: Instant,
    last_poll: Instant,
    poll_pending: bool,
    closing: bool,
    scrub: Option<f64>,
}
impl ReviewApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
            show_notes: false,
            snapshots: true,
            resume: true,
            active: None,
            starting: false,
            stop_requested: false,
            stopping: false,
            held_record: false,
            error,
            dirty: false,
            last_save: Instant::now(),
            last_poll: Instant::now(),
            poll_pending: false,
            closing: false,
            scrub: None,
        }
    }
    fn send(&mut self, command: serde_json::Value) {
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
            || self.opening.is_some()
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
                let screenshot = s.directory.join(format!("frames/{id}.png"));
                self.active = Some(id);
                self.starting = true;
                self.stop_requested = false;
                self.playing = false;
                self.show_notes = true;
                self.send(json!({"command":"record","id":id,"frame_index":self.anchor.frame_index,"audio":audio,"screenshot":if self.snapshots {Some(screenshot)} else {None}}));
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
                Err(e) => self.error = Some(e),
            }
        }
        while let Some(event) = self.backend.as_ref().and_then(|b| b.events.try_recv().ok()) {
            match event {
                Event::Opened { duration, frames } => {
                    self.duration = duration;
                    self.frames = frames;
                    self.ready = true;
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
                Event::Recording {
                    id,
                    anchor,
                    screenshot,
                } => {
                    if self.active == Some(id) {
                        self.starting = false;
                    }
                    if let Some(note) = self.session.as_mut().and_then(|s| s.note_mut(id)) {
                        note.anchor = anchor;
                        note.timestamp_seconds = anchor.seconds();
                        if screenshot {
                            note.screenshot = Some(format!("frames/{id}.png"));
                        }
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
                    if self.resume && !self.closing && !self.dirty {
                        self.send(json!({"command":"play"}));
                    }
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
                    if let Some(id) = id {
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
                    if operation == "service" {
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
    fn toggle_play(&mut self) {
        if self.ready && self.active.is_none() {
            self.send(json!({"command":if self.playing{"pause"}else{"play"}}));
            self.playing = !self.playing;
        }
    }
    fn step(&mut self, delta: i64) {
        if self.ready && self.active.is_none() {
            self.send(json!({"command":"step","delta":delta}));
            self.playing = false;
        }
    }
}
impl eframe::App for ReviewApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.drain(ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
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
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(Key::Space)) {
                self.toggle_play();
            }
            if ctx.input(|i| i.key_pressed(Key::ArrowLeft)) {
                self.step(-1);
            }
            if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
                self.step(1);
            }
            if ctx.input(|i| i.key_pressed(Key::R)) && self.active.is_none() {
                self.held_record = true;
                self.start_note();
            }
        }
        if self.held_record && ctx.input(|i| i.key_released(Key::R) || !i.focused) {
            self.held_record = false;
            self.stop_note();
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::O)) {
            self.open(ctx);
        }
        egui::TopBottomPanel::top("menu").show(ctx,|ui|{
            ui.horizontal(|ui|{
                ui.add_enabled_ui(self.active.is_none() && self.opening.is_none(),|ui|{
                    if ui.button("＋").on_hover_text("Open video · ⌘O").clicked(){self.open(ctx);}
                });
                if let Some(s)=&self.session{ui.weak(s.review.source.path.file_name().unwrap_or_default().to_string_lossy());}
                if self.opening.is_some() || (self.session.is_some()&&!self.ready && self.error.is_none()){ui.spinner();}
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                    ui.menu_button("⋯",|ui|{
                        ui.checkbox(&mut self.snapshots,"Capture frame with note");ui.checkbox(&mut self.resume,"Resume after recording");
                        if ui.button("Show review folder").clicked(){if let Some(s)=&self.session{let _=std::process::Command::new("open").arg(&s.directory).spawn();}}
                        ui.separator();ui.weak("Space: play / pause\n← →: previous / next frame\nHold R: record a note");
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
                    if ui.small_button("×").clicked() {
                        self.error = None;
                    }
                });
            });
        }
        egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.ready && self.active.is_none(), |ui| {
                    if ui.button("⏮").on_hover_text("Previous frame · ←").clicked() {
                        self.step(-1);
                    }
                    if ui
                        .button(if self.playing { "⏸" } else { "▶" })
                        .on_hover_text("Play / pause · Space")
                        .clicked()
                    {
                        self.toggle_play();
                    }
                    if ui.button("⏭").on_hover_text("Next frame · →").clicked() {
                        self.step(1);
                    }
                });
                ui.monospace(timecode(self.anchor.seconds()));
                ui.add_enabled_ui(self.ready && self.active.is_none(), |ui| {
                    let mut seconds = self.scrub.unwrap_or(self.anchor.seconds());
                    let width = (ui.available_width() - 160.0).max(80.0);
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
                            egui::RichText::new("■").color(Color32::RED)
                        } else {
                            egui::RichText::new("●")
                        };
                        if ui
                            .button(text)
                            .on_hover_text("Record / stop · Hold R")
                            .clicked()
                        {
                            if self.active.is_some() {
                                self.stop_note();
                            } else {
                                self.start_note();
                            }
                        }
                    },
                );
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
                                    if matches!(
                                        note.state,
                                        NoteState::Failed | NoteState::Interrupted
                                    ) && ui
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
                                if ui
                                    .add(
                                        egui::TextEdit::multiline(&mut note.transcript)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(3)
                                            .hint_text("Note"),
                                    )
                                    .changed()
                                {
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
                            self.backend.is_some() && self.opening.is_none(),
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
        if self.ready && !self.poll_pending && self.last_poll.elapsed() > Duration::from_millis(33)
        {
            self.send(json!({"command":"poll"}));
            self.poll_pending = true;
            self.last_poll = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
