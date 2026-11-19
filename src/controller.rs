//! Foreground-scoped input from the app's optional, private child.

use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationDidResignActiveNotification, NSView,
    NSWindowDidResignKeyNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::Deserialize;
use std::{
    fs,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_LINE: usize = 128;
const QUEUE: usize = 16;
const FRESH: Duration = Duration::from_millis(150);
const TICK: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Ct,
    Micro,
    MxCreative,
}

impl Device {
    const ALL: [Self; 3] = [Self::Ct, Self::Micro, Self::MxCreative];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ct => "Loupedeck CT",
            Self::Micro => "Creator Micro 2",
            Self::MxCreative => "MX Creative Console",
        }
    }

    fn arg(self) -> &'static str {
        match self {
            Self::Ct => "ct",
            Self::Micro => "micro",
            Self::MxCreative => "mx-creative",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    TogglePlay,
    SeekStart,
    SeekEnd,
    BackOneSecond,
    ForwardOneSecond,
    Screenshot,
    ToggleDictation,
    PreviousFrame,
    NextFrame,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    action: Action,
}

#[derive(Default)]
struct Lines {
    line: Vec<u8>,
    started: Option<Instant>,
}

impl Lines {
    fn push(
        &mut self,
        bytes: &[u8],
        now: Instant,
        mut emit: impl FnMut(Action, Instant) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        self.check_time(now)?;
        for &byte in bytes {
            let started = *self.started.get_or_insert(now);
            if byte == b'\n' {
                // Serde also accepts positional arrays for structs.
                if self.line.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
                    return Err("Controller sent an invalid action.");
                }
                let wire: Wire = serde_json::from_slice(&self.line)
                    .map_err(|_| "Controller sent an invalid action.")?;
                emit(wire.action, started)?;
                self.line.clear();
                self.started = None;
            } else if self.line.len() < MAX_LINE {
                self.line.push(byte);
            } else {
                return Err("Controller action exceeded the line limit.");
            }
        }
        Ok(())
    }

    fn check_time(&self, now: Instant) -> Result<(), &'static str> {
        if self.started.is_some_and(|t| now.duration_since(t) > FRESH) {
            Err("Controller action did not finish in time.")
        } else {
            Ok(())
        }
    }
}

fn is_executable(path: &Path) -> bool {
    path.is_absolute()
        && fs::symlink_metadata(path)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        && path.canonicalize().is_ok_and(|resolved| resolved == path)
}

fn bundled_helper(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let contents = dir.parent()?;
    let bundle = contents.parent()?;
    if exe.file_name()? != "fesreview"
        || dir.file_name()? != "MacOS"
        || contents.file_name()? != "Contents"
        || bundle.extension()? != "app"
    {
        return None;
    }
    let helper = dir.join("copilot-controller");
    is_executable(&helper).then_some(helper)
}

struct Input {
    action: Action,
    received: Instant,
}

struct Run {
    epoch: u64,
    cancel: Arc<AtomicBool>,
    input: Receiver<Input>,
    worker: JoinHandle<Result<(), &'static str>>,
}

impl Run {
    fn start(
        helper: PathBuf,
        device: Device,
        epoch: u64,
        focus_epoch: Arc<AtomicU64>,
    ) -> io::Result<Self> {
        let (tx, input) = mpsc::sync_channel(QUEUE);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = thread::Builder::new()
            .name("review-controller".into())
            .spawn(move || {
                run_child(
                    &helper,
                    device,
                    || stop.load(Relaxed) || focus_epoch.load(Relaxed) != epoch,
                    &tx,
                )
            })?;
        Ok(Self {
            epoch,
            cancel,
            input,
            worker,
        })
    }

    fn stop(&self) {
        self.cancel.store(true, Relaxed);
        self.worker.thread().unpark();
    }
}

fn run_child(
    helper: &Path,
    device: Device,
    cancelled: impl Fn() -> bool,
    tx: &SyncSender<Input>,
) -> Result<(), &'static str> {
    if cancelled() {
        return Ok(());
    }
    if !is_executable(helper) {
        return Err("Bundled controller helper is unavailable.");
    }
    let mut child = Command::new(helper)
        .args(["review-input", "--device", device.arg()])
        .env_clear()
        .current_dir(helper.parent().expect("absolute helper path"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not launch the bundled controller helper.")?;
    let result = (|| {
        let mut pipe = child.stdout.take().ok_or("Missing controller pipe.")?;
        // Only this worker owns the read end. Nonblocking reads make cancellation
        // independent of the child or any other writer closing its end.
        if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            return Err("Could not configure the controller pipe.");
        }
        let mut lines = Lines::default();
        let mut buf = [0; 512];
        while !cancelled() {
            if child
                .try_wait()
                .map_err(|_| "Could not monitor controller.")?
                .is_some()
            {
                return Err("Controller helper exited.");
            }
            let now = Instant::now();
            lines.check_time(now)?;
            match pipe.read(&mut buf) {
                Ok(0) => return Err("Controller closed its action stream."),
                Ok(n) => lines.push(&buf[..n], now, |action, received| {
                    tx.try_send(Input { action, received })
                        .map_err(|_| "Controller input queue overflowed.")
                })?,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::park_timeout(TICK),
                Err(_) => return Err("Could not read controller input."),
            }
        }
        Ok(())
    })();
    // Child::kill targets just this unreaped child, not a name or a process group.
    let _ = child.kill();
    child
        .wait()
        .map_err(|_| "Could not reap controller helper.")?;
    result
}

pub struct Bridge {
    helper: Option<PathBuf>,
    selected: Option<Device>,
    run: Option<Run>,
    error: Option<&'static str>,
    shutdown: Option<ShutdownSender>,
    focus_epoch: Arc<AtomicU64>,
}

pub type ShutdownSender = Sender<JoinHandle<Result<(), &'static str>>>;
pub type ShutdownReceiver = Receiver<JoinHandle<Result<(), &'static str>>>;

impl Bridge {
    pub fn new(shutdown: Option<ShutdownSender>) -> Self {
        let helper = std::env::current_exe()
            .and_then(fs::canonicalize)
            .ok()
            .and_then(|exe| bundled_helper(&exe));
        Self {
            helper,
            selected: None,
            run: None,
            error: None,
            shutdown,
            focus_epoch: Arc::default(),
        }
    }

    pub fn suspend(&self) {
        if let Some(run) = &self.run {
            run.stop();
        }
    }

    fn available(&self) -> bool {
        self.helper.is_some()
    }

    pub fn observe_focus(&self) -> Option<Focus> {
        self.available()
            .then(|| Focus::new(self.focus_epoch.clone()))
    }

    fn select(&mut self, device: Option<Device>) {
        self.suspend();
        self.error = None;
        self.selected = device;
    }

    pub fn menu(&mut self, ui: &mut eframe::egui::Ui) {
        if !self.available() {
            ui.weak("Controller unavailable: helper not bundled");
            return;
        }
        ui.menu_button("Foreground controller", |ui| {
            for device in Device::ALL {
                if ui
                    .selectable_label(self.selected == Some(device), device.label())
                    .clicked()
                {
                    self.select(Some(device));
                    ui.close();
                }
            }
            if ui
                .selectable_label(self.selected.is_none(), "Off")
                .clicked()
            {
                self.select(None);
                ui.close();
            }
            if let Some(error) = self.error {
                ui.label(error);
            } else if self.selected == Some(Device::MxCreative) {
                ui.weak(
                    "MX Creative input only: 7 buttons for play, ±1 frame, ±1 second, \
                     screenshot and dictation. Display/icons are not controlled.",
                );
            } else if self.selected.is_some() {
                ui.weak("Input only while this review window is ready and focused.");
            }
        });
    }

    pub fn error(&self) -> Option<&'static str> {
        self.error
    }

    // The receiver belongs to exactly one foreground generation. Cancel it even
    // when a loss and regain of focus both happened between UI updates.
    pub fn next(&mut self, foreground: Option<u64>) -> Option<Action> {
        let enabled = self.selected.zip(foreground);
        if let Some(run) = &self.run {
            if enabled.is_none_or(|(_, epoch)| epoch != run.epoch) {
                run.stop();
            }
            if run.worker.is_finished() {
                let run = self.run.take().unwrap();
                let result = run
                    .worker
                    .join()
                    .unwrap_or(Err("Controller worker failed."));
                if !run.cancel.load(Relaxed) {
                    self.select(None);
                    self.error = Some(result.err().unwrap_or("Controller disconnected."));
                    return None;
                }
            } else if run.cancel.load(Relaxed) {
                return None;
            } else {
                return match run.input.try_recv() {
                    Ok(input) if input.received.elapsed() <= FRESH => Some(input.action),
                    Ok(_) => {
                        self.select(None);
                        self.error = Some("Controller input expired. Select the device to retry.");
                        None
                    }
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
                };
            }
        }
        if let (Some((device, epoch)), Some(helper)) = (enabled, &self.helper) {
            match Run::start(helper.clone(), device, epoch, self.focus_epoch.clone()) {
                Ok(run) => self.run = Some(run),
                Err(_) => {
                    self.select(None);
                    self.error = Some("Could not start controller worker.");
                }
            }
        }
        None
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.suspend();
        if let (Some(run), Some(shutdown)) = (self.run.take(), &self.shutdown) {
            let _ = shutdown.send(run.worker);
        }
    }
}

pub fn finish_shutdown(shutdown: ShutdownReceiver) {
    if let Ok(worker) = shutdown.try_recv() {
        // The UI loop has already exited; wait for our child's SIGKILL/reap
        // rather than letting the app exit while its child is still alive.
        if !matches!(worker.join(), Ok(Ok(()))) {
            eprintln!("Controller worker reported an error during shutdown.");
        }
    }
}

pub struct Focus {
    epoch: Arc<AtomicU64>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

impl Focus {
    fn new(epoch: Arc<AtomicU64>) -> Self {
        let counter = epoch.clone();
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            counter.fetch_add(1, Relaxed);
        });
        let center = NSNotificationCenter::defaultCenter();
        // AppKit posts both notifications on the UI thread. The block captures
        // only an atomic and is also safe to invoke from any other thread.
        let observers = unsafe {
            [
                NSWindowDidResignKeyNotification,
                NSApplicationDidResignActiveNotification,
            ]
            .into_iter()
            .map(|name| {
                center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
            })
            .collect()
        };
        Self { epoch, observers }
    }

    pub fn foreground(&self, frame: &eframe::Frame) -> Option<u64> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = frame.window_handle().ok()?.as_raw() else {
            return None;
        };
        // The AppKit window handle borrows a live NSView from eframe's window.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window()?;
        let app = NSApplication::sharedApplication(mtm);
        (app.isActive()
            && app
                .keyWindow()
                .is_some_and(|key| std::ptr::eq(&*key, &*window))
            && app.modalWindow().is_none()
            && window.attachedSheet().is_none()
            && window.isVisible()
            && !window.isMiniaturized())
        .then(|| self.epoch.load(Relaxed))
    }
}

impl Drop for Focus {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // Each token came from this notification center.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn exact_actions_only_and_partial_reads() {
        use Action::*;
        for (name, expected) in [
            ("toggle_play", TogglePlay),
            ("seek_start", SeekStart),
            ("seek_end", SeekEnd),
            ("back_one_second", BackOneSecond),
            ("forward_one_second", ForwardOneSecond),
            ("screenshot", Screenshot),
            ("toggle_dictation", ToggleDictation),
            ("previous_frame", PreviousFrame),
            ("next_frame", NextFrame),
        ] {
            let mut lines = Lines::default();
            let now = Instant::now();
            let mut seen = Vec::new();
            let mut emit = |action, time| {
                seen.push((action, time));
                Ok(())
            };
            lines.push(b"{\"action\":", now, &mut emit).unwrap();
            lines
                .push(format!("\"{name}\"}}\n").as_bytes(), now + TICK, &mut emit)
                .unwrap();
            assert_eq!(seen, [(expected, now)]);
        }
        for invalid in [
            &b"{\"action\":\"record\"}\n"[..],
            b"{\"command\":\"toggle_play\"}\n",
            b"{\"action\":\"toggle_play\",\"extra\":true}\n",
            b"{\"action\":\"toggle_play\",\"action\":\"seek_end\"}\n",
            b"{\"action\":null}\n",
            b"[\"toggle_play\"]\n",
            b"{}\n",
            b"\n",
            b"{\"action\":\"toggle_play\"}{}\n",
            b"\xff\n",
        ] {
            assert!(Lines::default()
                .push(invalid, Instant::now(), |_, _| panic!(
                    "accepted invalid action"
                ))
                .is_err());
        }
    }

    #[test]
    fn wire_rejects_overlong_and_slow_records() {
        let now = Instant::now();
        let mut lines = Lines::default();
        let emit = |_, _| panic!("emitted an incomplete action");
        assert!(lines.push(&[b'x'; MAX_LINE + 1], now, emit).is_err());
        let mut lines = Lines::default();
        lines.push(b"{", now, emit).unwrap();
        assert!(lines.push(b"}\n", now + FRESH + TICK, emit).is_err());
    }

    #[test]
    fn devices_expose_exact_helper_selection() {
        assert_eq!(
            Device::ALL.map(|device| (device.label(), device.arg())),
            [
                ("Loupedeck CT", "ct"),
                ("Creator Micro 2", "micro"),
                ("MX Creative Console", "mx-creative"),
            ]
        );

        let mut bridge = Bridge {
            helper: None,
            selected: None,
            run: None,
            error: None,
            shutdown: None,
            focus_epoch: Arc::default(),
        };
        bridge.select(Some(Device::MxCreative));
        assert_eq!(bridge.selected, Some(Device::MxCreative));
        bridge.select(None);
        assert_eq!(bridge.selected, None);
    }

    // Keep a worker alive with a deliberately queued event, without sleeps to
    // order the simulated focus transition and the arrival of the event.
    pub(crate) fn queued(epoch: u64, received: Instant) -> Bridge {
        let (tx, input) = mpsc::sync_channel(QUEUE);
        tx.try_send(Input {
            action: Action::ToggleDictation,
            received,
        })
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = thread::spawn(move || {
            while !stop.load(Relaxed) {
                thread::park();
            }
            Ok(())
        });
        Bridge {
            helper: None,
            selected: Some(Device::Ct),
            run: Some(Run {
                epoch,
                cancel,
                input,
                worker,
            }),
            error: None,
            shutdown: None,
            focus_epoch: Arc::default(),
        }
    }

    #[test]
    fn discards_queued_input_when_inactive_refocused_or_expired() {
        let mut active = queued(3, Instant::now());
        assert_eq!(active.next(Some(3)), Some(Action::ToggleDictation));

        let mut background = queued(3, Instant::now());
        assert_eq!(background.next(None), None);
        assert_eq!(background.next(Some(4)), None);

        let mut fast_refocus = queued(3, Instant::now());
        assert_eq!(fast_refocus.next(Some(5)), None);
        assert_eq!(fast_refocus.next(Some(5)), None);

        let mut paused = queued(3, Instant::now());
        paused.suspend(); // e.g. dialog / session switch without a focus event.
        assert_eq!(paused.next(Some(3)), None);

        let mut stale = queued(3, Instant::now() - FRESH - TICK);
        assert_eq!(stale.next(Some(3)), None);
        assert!(stale.error().is_some());
    }

    #[test]
    fn native_loss_notifications_invalidate_even_between_frames() {
        let focus = Focus::new(Arc::default());
        assert_eq!(focus.epoch.load(Relaxed), 0);
        let center = NSNotificationCenter::defaultCenter();
        // No real window or application activation is needed.
        unsafe {
            center.postNotificationName_object(NSWindowDidResignKeyNotification, None);
            center.postNotificationName_object(NSApplicationDidResignActiveNotification, None);
        }
        assert_eq!(focus.epoch.load(Relaxed), 2);
    }

    #[test]
    fn bundled_fixture_runs_only_after_opt_in_and_restarts_after_refocus() {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&target).unwrap();
        let temp = tempfile::tempdir_in(target).unwrap();
        let bin = temp.path().join("Review.app/Contents/MacOS");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("fesreview");
        let helper = bin.join("copilot-controller");
        assert_eq!(bundled_helper(&exe), None);
        std::os::unix::fs::symlink(&exe, &helper).unwrap();
        assert_eq!(bundled_helper(&exe), None);
        fs::remove_file(&helper).unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        assert!(Command::new(compiler)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/controller.rs"
            ))
            .args(["-o", helper.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        let (tx, rx) = mpsc::channel();
        let mut bridge = Bridge::new(Some(tx));
        bridge.helper = bundled_helper(&exe);
        assert!(bridge.available());
        assert_eq!(bridge.next(Some(1)), None);
        assert!(bridge.run.is_none());
        bridge.select(Some(Device::Ct));
        assert_eq!(bridge.next(None), None);
        assert!(bridge.run.is_none());
        let next = |bridge: &mut Bridge, epoch| {
            let limit = Instant::now() + Duration::from_secs(4);
            while Instant::now() < limit {
                if let Some(action) = bridge.next(Some(epoch)) {
                    return action;
                }
                assert_eq!(bridge.error(), None);
                thread::sleep(TICK);
            }
            panic!("did not receive fixture action");
        };
        bridge.focus_epoch.store(1, Relaxed);
        assert_eq!(next(&mut bridge, 1), Action::TogglePlay);
        bridge.focus_epoch.store(2, Relaxed);
        // The worker shuts down independently of any background UI update.
        let limit = Instant::now() + Duration::from_secs(1);
        while !bridge.run.as_ref().unwrap().worker.is_finished() && Instant::now() < limit {
            thread::sleep(TICK);
        }
        assert!(bridge.run.as_ref().unwrap().worker.is_finished());
        assert_eq!(bridge.next(None), None);
        assert_eq!(next(&mut bridge, 2), Action::TogglePlay);
        bridge.select(Some(Device::Micro));
        assert_eq!(next(&mut bridge, 2), Action::NextFrame);
        bridge.select(Some(Device::MxCreative));
        assert_eq!(next(&mut bridge, 2), Action::BackOneSecond);
        drop(bridge);
        const REAP_GRACE: Duration = Duration::from_secs(1);
        let worker = rx.recv_timeout(REAP_GRACE).unwrap();
        let limit = Instant::now() + REAP_GRACE;
        while !worker.is_finished() && Instant::now() < limit {
            thread::sleep(TICK);
        }
        assert!(
            worker.is_finished(),
            "fixture was not terminated and reaped"
        );
        assert_eq!(worker.join().unwrap(), Ok(()));
    }
}
