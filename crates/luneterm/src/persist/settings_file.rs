//! `settings.json` (plan M10 step 5, D5, D17): the settings model to and from JSON.
//!
//! The file is `{"version": 1, <field>: <value>, ...}`. Reading is lenient per field and strict
//! about the file: a field of the wrong type, or an enum name nobody knows, takes its default and
//! the rest is kept; the numbers and texts go through [`Settings::clamped`]. A file that is not
//! JSON, is over the reader's limits, is not an object, has no integer `version`, or has a newer
//! one than this build knows is [`Reject`]ed whole (the store keeps a copy and the defaults run).
//!
//! Fields this build does not know are tolerated, and written back unchanged after the known ones,
//! so a newer build's settings are not lost by an older one that saved.

use lune_ui::json::{self, Value};

use super::json_write::{Json, WriteError};
use super::{Reject, read_version};
use crate::settings::{BlurLevel, GlassStyle, Motion, OutlineAnim, Settings};

/// The version this build writes and reads.
pub const SETTINGS_VERSION: i64 = 1;

/// The settings and what the file held that this build does not know.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SettingsDoc {
    pub settings: Settings,
    /// Top-level members this build has no field for, in file order.
    pub unknown: Vec<(String, Json)>,
}

/// The names this build writes, in the order it writes them.
const KEYS: [&str; 17] = [
    "glass",
    "menu_blur",
    "motion",
    "font",
    "font_size",
    "palette",
    "animate_outline",
    "outline",
    "outline_full_screen",
    "scrollback",
    "default_cwd",
    "shell",
    "shell_args",
    "report_iterm2",
    "confirm_close",
    "osc_clipboard",
    "option_as_meta",
];

fn member<'a>(members: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Replaces `field` with the value the member holds when it has the right type.
fn text(members: &[(String, Value)], key: &str, field: &mut String) {
    if let Some(Value::String(s)) = member(members, key) {
        field.clone_from(s);
    }
}

fn flag(members: &[(String, Value)], key: &str, field: &mut bool) {
    if let Some(Value::Bool(b)) = member(members, key) {
        *field = *b;
    }
}

fn number(members: &[(String, Value)], key: &str, field: &mut u32) {
    if let Some(v) = member(members, key)
        && let Ok(n) = v.as_int()
        && let Ok(n) = u32::try_from(n)
    {
        *field = n;
    }
}

fn named<T: Copy>(
    members: &[(String, Value)],
    key: &str,
    field: &mut T,
    from: fn(&str) -> Option<T>,
) {
    if let Some(Value::String(s)) = member(members, key)
        && let Some(v) = from(s)
    {
        *field = v;
    }
}

/// Reads a settings file's bytes. See the module docs for what is rejected.
pub fn parse_settings(bytes: &[u8]) -> Result<SettingsDoc, Reject> {
    let value = json::parse(bytes).map_err(|_| Reject::Unreadable)?;
    let Value::Object(members) = &value else {
        return Err(Reject::NotAnObject);
    };
    read_version(members, SETTINGS_VERSION)?;
    let mut s = Settings::default();
    named(members, "glass", &mut s.glass, GlassStyle::from_name);
    named(members, "menu_blur", &mut s.menu_blur, BlurLevel::from_name);
    named(members, "motion", &mut s.motion, Motion::from_name);
    text(members, "font", &mut s.font);
    number(members, "font_size", &mut s.font_size);
    text(members, "palette", &mut s.palette);
    flag(members, "animate_outline", &mut s.animate_outline);
    named(members, "outline", &mut s.outline, OutlineAnim::from_name);
    flag(members, "outline_full_screen", &mut s.outline_full_screen);
    number(members, "scrollback", &mut s.scrollback);
    text(members, "default_cwd", &mut s.default_cwd);
    text(members, "shell", &mut s.shell);
    text(members, "shell_args", &mut s.shell_args);
    flag(members, "report_iterm2", &mut s.report_iterm2);
    flag(members, "confirm_close", &mut s.confirm_close);
    flag(members, "osc_clipboard", &mut s.osc_clipboard);
    flag(members, "option_as_meta", &mut s.option_as_meta);
    let unknown = members
        .iter()
        .filter(|(k, _)| k != "version" && !KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), Json::from_value(v)))
        .collect();
    Ok(SettingsDoc {
        settings: s.clamped(),
        unknown,
    })
}

/// The text of a settings file: `version`, the known fields, then the unknown ones.
pub fn render_settings(doc: &SettingsDoc) -> Result<String, WriteError> {
    let s = doc.settings.clone().clamped();
    let int = |n: u32| Json::Int(i64::from(n));
    let mut members: Vec<(String, Json)> = vec![
        ("version".into(), Json::Int(SETTINGS_VERSION)),
        ("glass".into(), Json::Str(s.glass.name().into())),
        ("menu_blur".into(), Json::Str(s.menu_blur.name().into())),
        ("motion".into(), Json::Str(s.motion.name().into())),
        ("font".into(), Json::Str(s.font)),
        ("font_size".into(), int(s.font_size)),
        ("palette".into(), Json::Str(s.palette)),
        ("animate_outline".into(), Json::Bool(s.animate_outline)),
        ("outline".into(), Json::Str(s.outline.name().into())),
        (
            "outline_full_screen".into(),
            Json::Bool(s.outline_full_screen),
        ),
        ("scrollback".into(), int(s.scrollback)),
        ("default_cwd".into(), Json::Str(s.default_cwd)),
        ("shell".into(), Json::Str(s.shell)),
        ("shell_args".into(), Json::Str(s.shell_args)),
        ("report_iterm2".into(), Json::Bool(s.report_iterm2)),
        ("confirm_close".into(), Json::Bool(s.confirm_close)),
        ("osc_clipboard".into(), Json::Bool(s.osc_clipboard)),
        ("option_as_meta".into(), Json::Bool(s.option_as_meta)),
    ];
    members.extend(doc.unknown.iter().cloned());
    Json::Object(members).to_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_changed() -> Settings {
        Settings {
            glass: GlassStyle::LiquidRegular,
            menu_blur: BlurLevel::High,
            motion: Motion::Reduced,
            font: "Some Mono".into(),
            font_size: 17,
            palette: "Nord".into(),
            animate_outline: false,
            outline: OutlineAnim::Comet,
            outline_full_screen: true,
            scrollback: 123,
            default_cwd: "/tmp".into(),
            shell: "/bin/zsh".into(),
            shell_args: "-l -x".into(),
            report_iterm2: true,
            confirm_close: false,
            osc_clipboard: false,
            option_as_meta: true,
        }
    }

    #[test]
    fn every_field_round_trips() {
        for settings in [Settings::default(), all_changed()] {
            let doc = SettingsDoc {
                settings,
                unknown: Vec::new(),
            };
            let text = render_settings(&doc).expect("writes");
            assert_eq!(parse_settings(text.as_bytes()), Ok(doc));
        }
    }

    #[test]
    fn the_file_names_the_version_and_stores_enums_and_fonts_by_name() {
        let text = render_settings(&SettingsDoc {
            settings: all_changed(),
            unknown: Vec::new(),
        })
        .expect("writes");
        assert!(text.starts_with("{\"version\":1,"), "{text}");
        assert!(text.contains("\"glass\":\"liquid-regular\""));
        assert!(text.contains("\"font\":\"Some Mono\""));
        assert!(text.contains("\"palette\":\"Nord\""));
        assert!(!text.contains("keep_glass_inactive"));
    }

    #[test]
    fn a_wrong_typed_field_takes_its_default_and_keeps_the_rest() {
        let doc = parse_settings(
            br#"{"version":1,"font_size":"big","glass":7,"motion":"warp","scrollback":12.5,
                 "confirm_close":"no","shell":["zsh"],"osc_clipboard":false,"palette":"Nord"}"#,
        )
        .expect("a lenient read");
        let d = Settings::default();
        assert_eq!(doc.settings.font_size, d.font_size);
        assert_eq!(doc.settings.glass, d.glass);
        assert_eq!(
            doc.settings.motion, d.motion,
            "an unknown name is no variant"
        );
        assert_eq!(
            doc.settings.scrollback, d.scrollback,
            "a fraction is no integer"
        );
        assert!(doc.settings.confirm_close, "a string is no bool");
        assert_eq!(doc.settings.shell, d.shell);
        // The fields that were fine are kept.
        assert!(!doc.settings.osc_clipboard);
        assert_eq!(doc.settings.palette, "Nord");
    }

    #[test]
    fn numbers_and_texts_are_range_checked_through_clamped() {
        let doc = parse_settings(
            br#"{"version":1,"font_size":99,"scrollback":4000000000,"font":"a\u0007b",
                 "default_cwd":"/tmp/\u001b","shell_args":"ok"}"#,
        )
        .expect("read");
        let d = Settings::default();
        assert_eq!(doc.settings.font_size, d.font_size, "99 is outside 8..=32");
        assert_eq!(doc.settings.scrollback, 100_000, "capped");
        assert_eq!(doc.settings.font, d.font, "a control character resets it");
        assert_eq!(doc.settings.default_cwd, "");
        assert_eq!(doc.settings.shell_args, "ok");
        // Negative and out-of-u32 numbers are no numbers for these fields.
        let doc = parse_settings(br#"{"version":1,"font_size":-3,"scrollback":-1}"#).expect("read");
        assert_eq!(doc.settings, d);
    }

    #[test]
    fn the_whole_file_is_refused_when_it_is_not_a_settings_object() {
        for (bytes, why) in [
            (&b""[..], Reject::Unreadable),
            (b"{", Reject::Unreadable),
            (b"not json", Reject::Unreadable),
            (b"\xff\xfe\x00", Reject::Unreadable),
            (b"[1,2]", Reject::NotAnObject),
            (b"\"x\"", Reject::NotAnObject),
            (b"{}", Reject::MissingVersion),
            (br#"{"font_size":12}"#, Reject::MissingVersion),
            (br#"{"version":"1"}"#, Reject::MissingVersion),
            (br#"{"version":1.5}"#, Reject::MissingVersion),
            (br#"{"version":0}"#, Reject::OldVersion(0)),
            (br#"{"version":-4}"#, Reject::OldVersion(-4)),
            (br#"{"version":2}"#, Reject::NewerVersion(2)),
            (
                br#"{"version":9007199254740992}"#,
                Reject::NewerVersion(1 << 53),
            ),
            // a key twice is an error to the reader
            (br#"{"version":1,"version":1}"#, Reject::Unreadable),
        ] {
            assert_eq!(
                parse_settings(bytes),
                Err(why),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
        // Over the reader's input limit.
        let big = format!(r#"{{"version":1,"x":"{}"}}"#, "a".repeat(json::MAX_INPUT));
        assert_eq!(parse_settings(big.as_bytes()), Err(Reject::Unreadable));
        // Nesting past the reader's depth.
        let deep = format!(
            r#"{{"version":1,"x":{}1{}}}"#,
            "[".repeat(20),
            "]".repeat(20)
        );
        assert_eq!(parse_settings(deep.as_bytes()), Err(Reject::Unreadable));
    }

    #[test]
    fn unknown_fields_are_tolerated_and_written_back_after_the_known_ones() {
        let doc = parse_settings(
            br#"{"version":1,"from_the_future":{"a":[1,2,{"b":null}]},"font_size":14,"flag":true}"#,
        )
        .expect("read");
        assert_eq!(doc.settings.font_size, 14);
        let names: Vec<_> = doc.unknown.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, ["from_the_future", "flag"]);
        let text = render_settings(&doc).expect("writes");
        assert!(
            text.contains(r#""from_the_future":{"a":[1,2,{"b":null}]}"#),
            "{text}"
        );
        assert!(text.ends_with("\"flag\":true}\n"));
        assert_eq!(parse_settings(text.as_bytes()), Ok(doc));
    }

    #[test]
    fn a_hostile_setting_text_round_trips_or_is_reset_never_corrupting_the_file() {
        // Quotes, backslashes and the longest shell text the model allows.
        let doc = SettingsDoc {
            settings: Settings {
                shell: format!("\"{}\\", "q\"\\".repeat(300)),
                shell_args: "a\"b\\c".into(),
                ..Settings::default()
            },
            unknown: Vec::new(),
        };
        let text = render_settings(&doc).expect("writes");
        assert_eq!(parse_settings(text.as_bytes()), Ok(doc));
    }

    #[test]
    fn a_seeded_fuzz_of_bytes_never_panics_and_only_ever_gives_a_doc_or_a_reject() {
        // Mutations of a valid file and of random bytes: every input is Ok or a named reject.
        let valid = render_settings(&SettingsDoc {
            settings: all_changed(),
            unknown: Vec::new(),
        })
        .expect("writes")
        .into_bytes();
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut ok, mut rejected) = (0, 0);
        for round in 0..3000 {
            let mut bytes = if round % 3 == 0 {
                (0..(next() % 300)).map(|_| next() as u8).collect()
            } else {
                valid.clone()
            };
            if round % 3 != 0 {
                for _ in 0..=(next() % 4) {
                    let at = (next() as usize) % bytes.len();
                    match next() % 3 {
                        0 => bytes[at] = next() as u8,
                        1 => bytes.truncate(at),
                        _ => bytes.insert(at, next() as u8),
                    }
                    if bytes.is_empty() {
                        break;
                    }
                }
            }
            match parse_settings(&bytes) {
                Ok(doc) => {
                    ok += 1;
                    // Whatever it read is inside the model's ranges.
                    assert_eq!(doc.settings.clone().clamped(), doc.settings);
                }
                Err(_) => rejected += 1,
            }
        }
        assert!(ok > 0 && rejected > 0, "ok {ok}, rejected {rejected}");
    }
}
