//! Synthetic subprocess: checks argv and stdin, sends one action, stays alive.

use std::io::{self, Read, Write};
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let device = match args.as_slice() {
        [mode, flag, value]
            if mode == "review-input" && flag == "--device" && (value == "ct" || value == "micro") => {
            value.as_str()
        }
        _ => {
            eprintln!(
                "controller fixture: expected `review-input --device ct|micro`"
            );
            std::process::exit(2);
        }
    };

    // Block until the harness closes our stdin; the contents are irrelevant.
    if !matches!(io::stdin().read(&mut [0]), Ok(0)) {
        eprintln!("controller fixture: expected stdin EOF");
        std::process::exit(2);
    }

    let action = if device == "ct" {
        "toggle_play"
    } else {
        "next_frame"
    };

    let mut stdout = io::stdout();
    if let Err(e) = writeln!(stdout, r#"{{"action":"{action}"}}"#) {
        eprintln!("controller: failed writing action: {e}");
        std::process::exit(2);
    }
    if let Err(e) = stdout.flush() {
        eprintln!("controller: failed flushing stdout: {e}");
        std::process::exit(2);
    }

    // Stay alive indefinitely, simulating a persistent device connection,
    // until the test harness terminates this process.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
