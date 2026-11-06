use fesreview::model::FrameTime;
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
};

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Opened {
        duration: f64,
        frames: u64,
    },
    Frame {
        anchor: FrameTime,
        png: String,
        playing: bool,
    },
    Recording {
        id: uuid::Uuid,
        anchor: FrameTime,
    },
    LiveTranscript {
        id: uuid::Uuid,
        text: String,
    },
    Screenshot {
        id: uuid::Uuid,
    },
    Recorded {
        id: uuid::Uuid,
    },
    Transcript {
        id: uuid::Uuid,
        text: String,
    },
    TranscriptionStatus {
        id: uuid::Uuid,
        message: String,
    },
    Failed {
        id: Option<uuid::Uuid>,
        operation: String,
        message: String,
    },
}
pub struct Backend {
    pub events: Receiver<Event>,
    commands: Sender<Value>,
    child: Child,
}
impl Backend {
    pub fn start(ctx: eframe::egui::Context) -> Result<Self, String> {
        if !cfg!(target_os = "macos") {
            return Err(
                "Media playback requires macOS. The portable review core can be tested here."
                    .into(),
            );
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let helper = executable
            .parent()
            .ok_or("Missing application directory")?
            .join("fesreview-native");
        let mut child = Command::new(helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "Cannot start media service: {e}. Build the .app with scripts/build-macos.sh."
                )
            })?;
        let mut input = child.stdin.take().ok_or("Missing media input")?;
        let output = child.stdout.take().ok_or("Missing media output")?;
        let (commands, receiver) = mpsc::channel::<Value>();
        let (sender, events) = mpsc::channel();
        let writer_events = sender.clone();
        let writer_ctx = ctx.clone();
        std::thread::spawn(move || {
            for command in receiver {
                if let Err(e) = writeln!(input, "{command}").and_then(|_| input.flush()) {
                    let _ = writer_events.send(Event::Failed {
                        id: None,
                        operation: "service".into(),
                        message: e.to_string(),
                    });
                    writer_ctx.request_repaint();
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let event = match line {
                    Ok(line) => serde_json::from_str(&line).unwrap_or_else(|e| Event::Failed {
                        id: None,
                        operation: "protocol".into(),
                        message: e.to_string(),
                    }),
                    Err(e) => Event::Failed {
                        id: None,
                        operation: "service".into(),
                        message: e.to_string(),
                    },
                };
                if sender.send(event).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
            let _ = sender.send(Event::Failed {
                id: None,
                operation: "service".into(),
                message: "Media service stopped. Saved notes and audio are retained.".into(),
            });
            ctx.request_repaint();
        });
        Ok(Self {
            events,
            commands,
            child,
        })
    }
    pub fn send(&self, command: Value) -> Result<(), String> {
        self.commands.send(command).map_err(|e| e.to_string())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
