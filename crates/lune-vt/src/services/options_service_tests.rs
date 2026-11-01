//! Port of xterm.js src/common/services/OptionsService.test.ts @ f447274 (10 tests), plus tests
//! for the typed surface: defaults, one `Err` per validated key, normalisation and change
//! reporting. Upstream's events are `set` return values (module notes), so the three
//! change-notification describes assert on them.
//!
//! Record of the 10 upstream tests:
//!  - constructor / "uses default value if invalid constructor option values passed for
//!    cols/rows": replaced, `undefined` is unrepresentable in a typed `OptionValue`;
//!    `constructor_without_values_keeps_defaults` is the nearest check.
//!  - constructor / "uses values from constructor option values if correctly passed":
//!    `constructor_uses_values_passed`.
//!  - constructor / "uses default value if invalid constructor option value passed":
//!    `constructor_drops_invalid_value_and_keeps_default`.
//!  - constructor / "object.keys return the correct number of options":
//!    `option_key_count_is_21`.
//!  - setOption / "applies valid fontWeight option values" and "normalizes invalid fontWeight
//!    option values": replaced, fontWeight is a renderer option (D3).
//!  - onOptionChange / "should fire on any option change": `set_reports_the_changed_key`.
//!  - onSpecificOptionChange (listed twice upstream, identical):
//!    `specific_option_change_filter` (both).
//!  - onMultipleOptionChange / "should fire only for specific options":
//!    `multiple_option_change_filter`.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

fn service() -> OptionsService {
    OptionsService::new([])
}

#[test]
fn constructor_without_values_keeps_defaults() {
    let s = service();
    assert_eq!(s.options(), &TerminalOptions::default());
    assert_eq!(s.options().rows, 24);
    assert_eq!(s.options().cols, 80);
}

#[test]
fn constructor_uses_values_passed() {
    let s = OptionsService::new([OptionValue::Cols(80), OptionValue::Rows(25)]);
    assert_eq!(s.options().rows, 25);
    assert_eq!(s.options().cols, 80);
}

#[test]
fn constructor_drops_invalid_value_and_keeps_default() {
    let s = OptionsService::new([OptionValue::TabStopWidth(0)]);
    assert_eq!(
        s.options().tab_stop_width,
        TerminalOptions::default().tab_stop_width
    );
    assert_eq!(s.options().tab_stop_width, 8);
}

#[test]
fn constructor_keeps_later_valid_values_after_an_invalid_one() {
    let s = OptionsService::new([
        OptionValue::FastScrollSensitivity(-1.0),
        OptionValue::Scrollback(7),
    ]);
    assert_eq!(s.options().fast_scroll_sensitivity, 5.0);
    assert_eq!(s.options().scrollback, 7);
}

#[test]
fn constructor_normalises_like_set() {
    let s = OptionsService::new([
        OptionValue::WordSeparator(String::new()),
        OptionValue::Scrollback(usize::MAX),
        OptionValue::WindowsPty(None),
    ]);
    assert_eq!(s.options().word_separator, DEFAULT_WORD_SEPARATOR);
    assert_eq!(s.options().scrollback, 4_294_967_295);
    assert_eq!(s.options().windows_pty, WindowsPty::default());
}

#[test]
fn option_key_count_is_21() {
    assert_eq!(OptionKey::ALL.len(), 21);
    // Every key is distinct and has a name.
    for (i, a) in OptionKey::ALL.iter().enumerate() {
        assert!(!a.name().is_empty());
        for b in &OptionKey::ALL[i + 1..] {
            assert_ne!(a, b);
            assert_ne!(a.name(), b.name());
        }
    }
}

#[test]
fn defaults_match_default_options() {
    // OptionsService.ts:12-60 (and DEFAULT_OPTIONS.windowOptions/windowsPty = {}).
    let d = TerminalOptions::default();
    assert_eq!(d.cols, 80);
    assert_eq!(d.rows, 24);
    assert_eq!(d.scrollback, 1000);
    assert_eq!(d.tab_stop_width, 8);
    assert!(!d.reflow_cursor_line);
    assert!(!d.windows_mode);
    assert_eq!(d.windows_pty.backend, None);
    assert_eq!(d.windows_pty.build_number, None);
    assert!(!d.convert_eol);
    assert_eq!(d.term_name, "xterm");
    assert_eq!(d.word_separator, " ()[]{}',\"`");
    assert!(!d.scroll_on_erase_in_display);
    assert!(d.scroll_on_user_input);
    assert!(!d.disable_stdin);
    assert!(!d.cursor_blink);
    assert_eq!(d.cursor_style, CursorStyle::Block);
    assert!(!d.screen_reader_mode);
    assert_eq!(d.window_options, WindowOptions::default());
    assert_eq!(d.log_level, LogLevel::Info);
    assert!(!d.allow_proposed_api);
    assert_eq!(d.fast_scroll_sensitivity, 5.0);
    assert_eq!(d.scroll_sensitivity, 1.0);
}

#[test]
fn window_options_default_has_every_flag_off() {
    let w = WindowOptions::default();
    // Upstream reads `windowOptions.<flag>` as undefined (falsy) for every flag.
    assert!(
        !(w.restore_win
            || w.minimize_win
            || w.set_win_position
            || w.set_win_size_pixels
            || w.raise_win
            || w.lower_win
            || w.refresh_win
            || w.set_win_size_chars
            || w.maximize_win
            || w.fullscreen_win
            || w.get_win_state
            || w.get_win_position
            || w.get_win_size_pixels
            || w.get_screen_size_pixels
            || w.get_cell_size_pixels
            || w.get_win_size_chars
            || w.get_screen_size_chars
            || w.get_icon_title
            || w.get_win_title
            || w.push_title
            || w.pop_title
            || w.set_win_lines)
    );
}

#[test]
fn set_reports_the_changed_key() {
    // onOptionChange: fires with the key on a change, and each key separately.
    let mut s = service();
    assert_eq!(
        s.set(OptionValue::CursorBlink(true)),
        Ok(Some(OptionKey::CursorBlink))
    );
    assert!(s.options().cursor_blink);
    assert_eq!(
        s.set(OptionValue::Scrollback(20)),
        Ok(Some(OptionKey::Scrollback))
    );
    assert_eq!(s.options().scrollback, 20);
}

#[test]
fn set_to_the_same_value_reports_nothing() {
    // "Don't fire an option change event if they didn't change" (:134).
    let mut s = service();
    assert_eq!(s.set(OptionValue::Scrollback(1000)), Ok(None));
    assert_eq!(
        s.set(OptionValue::Scrollback(20)),
        Ok(Some(OptionKey::Scrollback))
    );
    assert_eq!(s.set(OptionValue::Scrollback(20)), Ok(None));
    assert_eq!(
        s.set(OptionValue::CursorStyle(CursorStyle::Block)),
        Ok(None)
    );
    assert_eq!(
        s.set(OptionValue::CursorStyle(CursorStyle::Bar)),
        Ok(Some(OptionKey::CursorStyle))
    );
    assert_eq!(s.options().cursor_style, CursorStyle::Bar);
}

#[test]
fn specific_option_change_filter() {
    // onSpecificOptionChange('scrollback', listener): the listener runs only for scrollback
    // and reads the new value.
    let mut s = service();
    let mut seen = Vec::new();
    for value in [OptionValue::TabStopWidth(10), OptionValue::Scrollback(20)] {
        if let Some(key) = s.set(value).unwrap() {
            if key == OptionKey::Scrollback {
                seen.push(s.options().scrollback);
            }
        }
    }
    assert_eq!(seen, vec![20]);
}

#[test]
fn multiple_option_change_filter() {
    // onMultipleOptionChange(['scrollback'], listener).
    let mut s = service();
    let keys = [OptionKey::Scrollback];
    let mut called = false;
    if let Some(key) = s.set(OptionValue::TabStopWidth(10)).unwrap() {
        called |= key.is_one_of(&keys);
    }
    assert!(!called);
    if let Some(key) = s.set(OptionValue::Scrollback(20)).unwrap() {
        called |= key.is_one_of(&keys);
    }
    assert!(called);
}

#[test]
fn every_key_round_trips_through_set() {
    // One non-default value per key: `set` reports exactly that key and stores the value.
    let mut s = service();
    let values = [
        OptionValue::Cols(100),
        OptionValue::Rows(30),
        OptionValue::Scrollback(5),
        OptionValue::TabStopWidth(4),
        OptionValue::ReflowCursorLine(true),
        OptionValue::WindowsMode(true),
        OptionValue::WindowsPty(Some(WindowsPty {
            backend: Some(PtyBackendKind::Conpty),
            build_number: Some(19045),
        })),
        OptionValue::ConvertEol(true),
        OptionValue::TermName("vt100".to_owned()),
        OptionValue::WordSeparator(" ".to_owned()),
        OptionValue::ScrollOnEraseInDisplay(true),
        OptionValue::ScrollOnUserInput(false),
        OptionValue::DisableStdin(true),
        OptionValue::CursorBlink(true),
        OptionValue::CursorStyle(CursorStyle::Underline),
        OptionValue::ScreenReaderMode(true),
        OptionValue::WindowOptions(WindowOptions {
            set_win_lines: true,
            ..WindowOptions::default()
        }),
        OptionValue::LogLevel(LogLevel::Off),
        OptionValue::AllowProposedApi(true),
        OptionValue::FastScrollSensitivity(2.5),
        OptionValue::ScrollSensitivity(3.0),
    ];
    assert_eq!(values.len(), OptionKey::ALL.len());
    let mut keys = Vec::new();
    for value in values {
        let key = value.key();
        assert_eq!(s.set(value), Ok(Some(key)));
        keys.push(key);
    }
    // The 21 values cover the 21 keys exactly once.
    for key in OptionKey::ALL {
        assert_eq!(
            keys.iter().filter(|k| **k == key).count(),
            1,
            "{}",
            key.name()
        );
    }
    let o = s.options();
    assert_eq!(
        (o.cols, o.rows, o.scrollback, o.tab_stop_width),
        (100, 30, 5, 4)
    );
    assert!(o.reflow_cursor_line && o.windows_mode && o.convert_eol);
    assert_eq!(o.windows_pty.backend, Some(PtyBackendKind::Conpty));
    assert_eq!(o.windows_pty.build_number, Some(19045));
    assert_eq!(o.term_name, "vt100");
    assert_eq!(o.word_separator, " ");
    assert!(o.scroll_on_erase_in_display && !o.scroll_on_user_input && o.disable_stdin);
    assert!(o.cursor_blink && o.screen_reader_mode && o.allow_proposed_api);
    assert_eq!(o.cursor_style, CursorStyle::Underline);
    assert!(o.window_options.set_win_lines && !o.window_options.get_win_title);
    assert_eq!(o.log_level, LogLevel::Off);
    assert_eq!(
        (o.fast_scroll_sensitivity, o.scroll_sensitivity),
        (2.5, 3.0)
    );
    // And a second identical round reports nothing.
    assert_eq!(s.set(OptionValue::Cols(100)), Ok(None));
}

#[test]
fn empty_word_separator_stores_the_default() {
    let mut s = service();
    s.set(OptionValue::WordSeparator("-".to_owned())).unwrap();
    assert_eq!(s.options().word_separator, "-");
    // Empty is falsy upstream and falls back to the default (:158-160): a change from "-".
    assert_eq!(
        s.set(OptionValue::WordSeparator(String::new())),
        Ok(Some(OptionKey::WordSeparator))
    );
    assert_eq!(s.options().word_separator, DEFAULT_WORD_SEPARATOR);
    // Already the default: no change.
    assert_eq!(s.set(OptionValue::WordSeparator(String::new())), Ok(None));
}

#[test]
fn scrollback_is_clamped_to_max_buffer_size() {
    let mut s = service();
    assert_eq!(
        s.set(OptionValue::Scrollback(4_294_967_295)),
        Ok(Some(OptionKey::Scrollback))
    );
    assert_eq!(s.options().scrollback, 4_294_967_295);
    // Above the cap clamps to it, so it is the same value again.
    assert_eq!(s.set(OptionValue::Scrollback(usize::MAX)), Ok(None));
    assert_eq!(s.options().scrollback, 4_294_967_295);
    // 0 is allowed (`< 0` is the error, and unsigned values cannot be below 0).
    assert_eq!(
        s.set(OptionValue::Scrollback(0)),
        Ok(Some(OptionKey::Scrollback))
    );
    assert_eq!(s.options().scrollback, 0);
}

#[test]
fn windows_pty_none_stores_the_empty_object() {
    let mut s = service();
    let conpty = WindowsPty {
        backend: Some(PtyBackendKind::Conpty),
        build_number: Some(21376),
    };
    s.set(OptionValue::WindowsPty(Some(conpty))).unwrap();
    assert_eq!(s.options().windows_pty, conpty);
    // `value ?? {}` (:202-204)
    assert_eq!(
        s.set(OptionValue::WindowsPty(None)),
        Ok(Some(OptionKey::WindowsPty))
    );
    assert_eq!(s.options().windows_pty, WindowsPty::default());
}

#[test]
fn tab_stop_width_below_one_is_an_error_and_keeps_the_old_value() {
    let mut s = service();
    let err = s.set(OptionValue::TabStopWidth(0)).unwrap_err();
    assert_eq!(
        err,
        OptionError::LessThanOne {
            key: OptionKey::TabStopWidth,
            value: 0
        }
    );
    assert_eq!(
        err.to_string(),
        "tabStopWidth cannot be less than 1, value: 0"
    );
    assert_eq!(s.options().tab_stop_width, 8);
    assert_eq!(
        s.set(OptionValue::TabStopWidth(1)),
        Ok(Some(OptionKey::TabStopWidth))
    );
}

#[test]
fn scroll_sensitivities_at_or_below_zero_are_errors() {
    let mut s = service();
    for (make, key, name) in [
        (
            OptionValue::FastScrollSensitivity as fn(f64) -> OptionValue,
            OptionKey::FastScrollSensitivity,
            "fastScrollSensitivity",
        ),
        (
            OptionValue::ScrollSensitivity,
            OptionKey::ScrollSensitivity,
            "scrollSensitivity",
        ),
    ] {
        for bad in [0.0, -0.5, -3.0, f64::NEG_INFINITY] {
            let err = s.set(make(bad)).unwrap_err();
            assert_eq!(err, OptionError::NotPositive { key, value: bad });
            assert_eq!(
                err.to_string(),
                format!("{name} cannot be less than or equal to 0, value: {bad}")
            );
        }
        // Small positive values are fine.
        assert_eq!(s.set(make(0.001)), Ok(Some(key)));
    }
    assert_eq!(s.options().fast_scroll_sensitivity, 0.001);
    assert_eq!(s.options().scroll_sensitivity, 0.001);
}

#[test]
fn rejected_values_store_nothing() {
    let mut s = service();
    let before = s.options().clone();
    assert!(s.set(OptionValue::TabStopWidth(0)).is_err());
    assert!(s.set(OptionValue::FastScrollSensitivity(0.0)).is_err());
    assert!(s.set(OptionValue::ScrollSensitivity(-1.0)).is_err());
    assert_eq!(s.options(), &before);
}

#[test]
fn nan_sensitivity_passes_and_always_reports_a_change() {
    // NaN <= 0 is false in JS and in Rust: upstream accepts it, and `NaN !== NaN` fires every
    // time (module notes).
    let mut s = service();
    assert_eq!(
        s.set(OptionValue::ScrollSensitivity(f64::NAN)),
        Ok(Some(OptionKey::ScrollSensitivity))
    );
    assert_eq!(
        s.set(OptionValue::ScrollSensitivity(f64::NAN)),
        Ok(Some(OptionKey::ScrollSensitivity))
    );
}

#[test]
fn error_implements_std_error() {
    fn takes(_: &dyn std::error::Error) {}
    takes(&OptionError::LessThanOne {
        key: OptionKey::TabStopWidth,
        value: 0,
    });
}
