//! A terminal UI that retargets local Codex history sessions to the provider
//! currently configured in `config.toml`.

// This tool rewrites another application's database; keep it verifiably safe.
#![forbid(unsafe_code)]

mod app;
mod core;
mod ui;

use std::io;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Key};

/// How long to wait for input before checking on the worker threads again.
const TICK: Duration = Duration::from_millis(100);

fn main() {
    if std::env::args().nth(1).is_some() {
        eprintln!("codex-retarget 是全屏终端界面，不接受任何命令行参数。直接运行即可。");
        std::process::exit(2);
    }

    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("codex-retarget 需要在交互式终端中运行：{error}");
            std::process::exit(1);
        }
    };

    let outcome = run(&mut terminal);
    ratatui::restore();

    if let Err(error) = outcome {
        eprintln!("codex-retarget 已退出：{error}");
        std::process::exit(1);
    }
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::new();
    // Repaint on the first frame and whenever something actually happened.
    let mut dirty = true;

    while !app.should_quit {
        if app.poll() > 0 {
            dirty = true;
        }

        if dirty {
            terminal.draw(|frame| ui::draw(frame, &app))?;
            dirty = false;
        }

        if event::poll(TICK)? {
            match event::read()? {
                // Windows reports both Press and Release for every keystroke.
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(key) = map_key(key) {
                        app.handle_key(key);
                        dirty = true;
                    }
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }
    }

    Ok(())
}

fn map_key(key: KeyEvent) -> Option<Key> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    Some(match key.code {
        // Raw mode swallows the usual Ctrl+C interrupt, so wire it up by hand.
        KeyCode::Char('c') if control => Key::Char('q'),
        KeyCode::Char(character) if !control && !alt => Key::Char(character),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab => Key::Tab,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Option<Key> {
        map_key(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn plain_characters_are_forwarded() {
        assert_eq!(
            press(KeyCode::Char('m'), KeyModifiers::NONE),
            Some(Key::Char('m'))
        );
        assert_eq!(
            press(KeyCode::Char(' '), KeyModifiers::NONE),
            Some(Key::Char(' '))
        );
    }

    #[test]
    fn ctrl_c_quits_and_other_control_chords_are_dropped() {
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Key::Char('q'))
        );
        assert_eq!(press(KeyCode::Char('x'), KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('x'), KeyModifiers::ALT), None);
    }

    #[test]
    fn navigation_and_editing_keys_map_through() {
        assert_eq!(press(KeyCode::Enter, KeyModifiers::NONE), Some(Key::Enter));
        assert_eq!(press(KeyCode::Esc, KeyModifiers::NONE), Some(Key::Esc));
        assert_eq!(
            press(KeyCode::Backspace, KeyModifiers::NONE),
            Some(Key::Backspace)
        );
        assert_eq!(press(KeyCode::Up, KeyModifiers::NONE), Some(Key::Up));
        assert_eq!(press(KeyCode::Down, KeyModifiers::NONE), Some(Key::Down));
        assert_eq!(press(KeyCode::Home, KeyModifiers::NONE), Some(Key::Home));
        assert_eq!(press(KeyCode::End, KeyModifiers::NONE), Some(Key::End));
        assert_eq!(press(KeyCode::PageUp, KeyModifiers::NONE), Some(Key::PageUp));
        assert_eq!(
            press(KeyCode::PageDown, KeyModifiers::NONE),
            Some(Key::PageDown)
        );
        assert_eq!(press(KeyCode::Tab, KeyModifiers::NONE), Some(Key::Tab));
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        assert_eq!(press(KeyCode::F(5), KeyModifiers::NONE), None);
        assert_eq!(press(KeyCode::Insert, KeyModifiers::NONE), None);
    }
}
