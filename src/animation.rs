use std::{
    io::{self, BufWriter, Write},
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{audio::AudioCapture, render::Frame, terminal};

const FRAMES_PER_SECOND: u32 = 60;
const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / FRAMES_PER_SECOND as u64);

pub fn run(audio_enabled: bool) -> io::Result<()> {
    // Start capture before entering the alternate screen so permission and setup errors
    // remain readable in the user's normal terminal history.
    let audio = audio_enabled.then(AudioCapture::start).transpose()?;
    let mut stdout = io::stdout();
    let _terminal = terminal::Session::enter(&mut stdout)?;
    let mut output = BufWriter::new(stdout.lock());

    animate(&mut output, audio.as_ref())
}

fn animate(output: &mut impl Write, audio: Option<&AudioCapture>) -> io::Result<()> {
    let started_at = Instant::now();
    let mut next_frame_at = started_at;
    let dimensions = crossterm::terminal::size()?;
    let mut frame = Frame::new(dimensions.0, dimensions.1);
    let mut presenter = terminal::Presenter::new();

    loop {
        let now = Instant::now();

        if now >= next_frame_at {
            let elapsed = now.duration_since(started_at);
            if let Some(audio) = audio {
                let audio_frame = audio.snapshot();
                frame.render_with_audio(elapsed, &audio_frame);
            } else {
                frame.render_at(elapsed);
            }
            presenter.present(output, &frame)?;

            next_frame_at += FRAME_INTERVAL;
            if next_frame_at <= now {
                next_frame_at = now + FRAME_INTERVAL;
            }
        }

        let wait_time = next_frame_at.saturating_duration_since(Instant::now());
        if event::poll(wait_time)? {
            match event::read()? {
                Event::Key(key) if is_quit_key(key) => return Ok(()),
                Event::Resize(width, height) => {
                    frame = Frame::new(width, height);
                    presenter.invalidate();
                    next_frame_at = Instant::now();
                }
                _ => {}
            }
        }
    }
}

fn is_quit_key(key: KeyEvent) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }

    match key.code {
        KeyCode::Esc => true,
        KeyCode::Char(character) if character.eq_ignore_ascii_case(&'q') => true,
        KeyCode::Char(character) => {
            character.eq_ignore_ascii_case(&'c') && key.modifiers.contains(KeyModifiers::CONTROL)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::is_quit_key;

    #[test]
    fn expected_keys_quit() {
        assert!(is_quit_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        assert!(is_quit_key(KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT
        )));
        assert!(is_quit_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(is_quit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn ordinary_keys_do_not_quit() {
        assert!(!is_quit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
        assert!(!is_quit_key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn key_release_does_not_quit() {
        let released_q = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };

        assert!(!is_quit_key(released_q));
    }
}
