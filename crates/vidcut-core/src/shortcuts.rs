use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Play,
    Mark,
    Edit,
    Clips,
    Trim,
    App,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Play,
        Group::Mark,
        Group::Edit,
        Group::Clips,
        Group::Trim,
        Group::App,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Play => "Play and move",
            Group::Mark => "Mark",
            Group::Edit => "Put clips in",
            Group::Clips => "Change clips",
            Group::Trim => "Select and trim",
            Group::App => "Project and app",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Panel {
    App,
    Transcription,
}

macro_rules! actions {
    ($($variant:ident $name:literal,)*) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        pub enum Action {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$variant,)*];

            pub fn name(self) -> &'static str {
                match self {
                    $(Action::$variant => $name,)*
                }
            }
        }
    };
}

actions! {
    PlayOrStop "play_or_stop",
    PlayBackward "play_backward",
    Stop "stop",
    PlayForward "play_forward",
    FrameBack "frame_back",
    FrameForward "frame_forward",
    SecondBack "second_back",
    SecondForward "second_forward",
    PreviousCut "previous_cut",
    NextCut "next_cut",
    PreviousMarker "previous_marker",
    NextMarker "next_marker",
    TimelineStart "timeline_start",
    TimelineEnd "timeline_end",
    GoToTimecode "go_to_timecode",
    JumpForward "jump_forward",
    JumpBack "jump_back",
    TimelineOrSource "timeline_or_source",
    AudioScrubbing "audio_scrubbing",
    Loop "loop",
    Skim "skim",
    FullScreen "full_screen",
    EnhancedViewer "enhanced_viewer",
    LeaveFullScreen "leave_full_screen",
    MarkIn "mark_in",
    MarkOut "mark_out",
    GoToIn "go_to_in",
    GoToOut "go_to_out",
    ClearMarks "clear_marks",
    AddMarker "add_marker",
    DeleteMarker "delete_marker",
    SmartInsert "smart_insert",
    Overwrite "overwrite",
    RippleOverwrite "ripple_overwrite",
    PlaceOnTop "place_on_top",
    Append "append",
    Split "split",
    Dissolve "dissolve",
    Delete "delete",
    Lift "lift",
    Copy "copy",
    Cut "cut",
    Paste "paste",
    PictureOff "picture_off",
    Transform "transform",
    Crop "crop",
    DynamicZoom "dynamic_zoom",
    VoiceIsolation "voice_isolation",
    Ducking "ducking",
    ChangeDuration "change_duration",
    ChangeSpeed "change_speed",
    EditSubtitle "edit_subtitle",
    NextKeyframe "next_keyframe",
    PreviousKeyframe "previous_keyframe",
    AddKeyframe "add_keyframe",
    DeleteKeyframe "delete_keyframe",
    KeepStripe "keep_stripe",
    SelectAll "select_all",
    SelectNothing "select_nothing",
    SelectAtPlayhead "select_at_playhead",
    NearestCut "nearest_cut",
    CutSide "cut_side",
    TrimLeft "trim_left",
    TrimRight "trim_right",
    TrimSecondLeft "trim_second_left",
    TrimSecondRight "trim_second_right",
    SelectSentence "select_sentence",
    DynamicTrim "dynamic_trim",
    Snapping "snapping",
    Undo "undo",
    Redo "redo",
    Import "import",
    NewBin "new_bin",
    Preferences "preferences",
    Help "help",
}

impl Action {
    pub fn panel(self) -> Panel {
        match self {
            Action::SelectSentence => Panel::Transcription,
            _ => Panel::App,
        }
    }

    pub fn title(self) -> &'static str {
        SHORTCUTS
            .iter()
            .find(|s| matches!(s.keys, Keys::Own(a, _) if a == self))
            .map_or(self.name(), |s| s.title)
    }

    fn named(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }

    pub fn default_keys(self) -> Vec<Combo> {
        SHORTCUTS
            .iter()
            .find_map(|s| match s.keys {
                Keys::Own(a, keys) if a == self => Some(keys),
                _ => None,
            })
            .unwrap_or_default()
            .iter()
            .map(|k| k.parse().expect("the table holds valid keys"))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keys {
    Own(Action, &'static [&'static str]),
    Same(Action),
    Hold(Action, Action),
    Pointer(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub group: Group,
    pub keys: Keys,
    pub title: &'static str,
    pub explanation: Option<&'static str>,
}

#[cfg(target_os = "macos")]
pub fn key_name(key: &str) -> &str {
    match key {
        "Ctrl" => "Cmd",
        "Alt" => "Option",
        _ => key,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn key_name(key: &str) -> &str {
    key
}

const SIGNS: [(&str, &str); 18] = [
    (",", "Comma"),
    (".", "Period"),
    ("=", "Equals"),
    ("+", "Plus"),
    ("-", "Minus"),
    ("/", "Slash"),
    ("\\", "Backslash"),
    ("[", "OpenBracket"),
    ("]", "CloseBracket"),
    (";", "Semicolon"),
    ("'", "Quote"),
    ("`", "Backtick"),
    (":", "Colon"),
    ("?", "Questionmark"),
    ("!", "Exclamationmark"),
    ("|", "Pipe"),
    ("{", "OpenCurlyBracket"),
    ("}", "CloseCurlyBracket"),
];

const NAMED_KEYS: &[&str] = &[
    "Space",
    "Enter",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Left",
    "Right",
    "Up",
    "Down",
    "Escape",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
    "F25",
    "F26",
    "F27",
    "F28",
    "F29",
    "F30",
    "F31",
    "F32",
    "F33",
    "F34",
    "F35",
    "BrowserBack",
    "BrowserForward",
    "BrowserRefresh",
    "BrowserSearch",
    "BrowserHome",
    "BrowserFavorites",
    "BrowserStop",
    "MediaPlayPause",
    "MediaTrackNext",
    "MediaTrackPrevious",
    "MediaStop",
    "AudioVolumeMute",
    "AudioVolumeDown",
    "AudioVolumeUp",
    "IntlBackslash",
    "PrintScreen",
    "Pause",
    "Menu",
    "Help",
    "Clear",
    "CapsLock",
    "NumLock",
    "ScrollLock",
    "LaunchMail",
    "LaunchApp1",
    "LaunchApp2",
    "Fn",
    "Eject",
    "Power",
    "Sleep",
];

pub fn key_named(name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("Esc") {
        return Some("Escape");
    }
    SIGNS
        .iter()
        .find(|(sign, long)| *sign == name || long.eq_ignore_ascii_case(name))
        .map(|(sign, _)| *sign)
        .or_else(|| {
            NAMED_KEYS
                .iter()
                .find(|k| k.eq_ignore_ascii_case(name))
                .copied()
        })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Held {
    pub ctrl: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Combo {
    pub held: Held,
    pub key: &'static str,
}

impl Combo {
    pub fn parts(&self) -> Vec<&'static str> {
        let h = self.held;
        [
            (h.ctrl, "Ctrl"),
            (h.control, "Control"),
            (h.alt, "Alt"),
            (h.shift, "Shift"),
            (true, self.key),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name)
        .collect()
    }

    pub fn shown(&self) -> String {
        self.parts()
            .into_iter()
            .map(key_name)
            .collect::<Vec<_>>()
            .join("+")
    }
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.parts().join("+"))
    }
}

impl FromStr for Combo {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let mut held = Held::default();
        let mut rest = text.trim();
        while let Some((name, after)) = rest.split_once('+')
            && !after.is_empty()
        {
            match name.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => held.ctrl = true,
                "control" if cfg!(target_os = "macos") => held.control = true,
                "control" => held.ctrl = true,
                "alt" | "option" => held.alt = true,
                "shift" => held.shift = true,
                _ => {
                    return Err(Error::usage(format!(
                        "{text:?}: {name:?} is not Ctrl, Alt or Shift"
                    )));
                }
            }
            rest = after;
        }
        let key = key_named(rest)
            .ok_or_else(|| Error::usage(format!("{text:?}: there is no key named {rest:?}")))?;
        Ok(Self { held, key })
    }
}

impl Serialize for Combo {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Combo {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

pub type Custom = BTreeMap<Action, Vec<Combo>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Bindings {
    keys: BTreeMap<Action, Vec<Combo>>,
    owners: BTreeMap<(Panel, Combo), Action>,
    custom: Custom,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::of(&Custom::new()).expect("the table gives each key to one action")
    }
}

impl Bindings {
    pub fn of(custom: &Custom) -> Result<Self> {
        let mut keys = BTreeMap::new();
        let mut owners = BTreeMap::new();
        for &action in Action::ALL {
            let combos = match custom.get(&action) {
                Some(combos) => combos.clone(),
                None => action.default_keys(),
            };
            for &combo in &combos {
                match owners.insert((action.panel(), combo), action) {
                    Some(other) if other != action => {
                        return Err(Error::usage(format!(
                            "{combo} belongs to both {} and {}: one key has one action",
                            other.title(),
                            action.title(),
                        )));
                    }
                    _ => {}
                }
            }
            keys.insert(action, combos);
        }
        Ok(Self {
            keys,
            owners,
            custom: custom.clone(),
        })
    }

    pub fn keys(&self, action: Action) -> &[Combo] {
        self.keys.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn owner(&self, panel: Panel, combo: Combo) -> Option<Action> {
        self.owners.get(&(panel, combo)).copied()
    }

    pub fn is_custom(&self, action: Action) -> bool {
        self.custom.contains_key(&action)
    }

    pub fn any_custom(&self) -> bool {
        !self.custom.is_empty()
    }

    pub fn first(&self, action: Action) -> Option<String> {
        self.keys(action).first().map(Combo::shown)
    }

    pub fn first_or_none(&self, action: Action) -> String {
        self.first(action).unwrap_or_else(|| "no key".into())
    }

    pub fn named(&self, label: &str, action: Action) -> String {
        match self.first(action) {
            Some(key) => format!("{label}  ({key})"),
            None => label.to_string(),
        }
    }

    pub fn row(&self, s: &Shortcut) -> Vec<Vec<&'static str>> {
        match s.keys {
            Keys::Own(action, _) | Keys::Same(action) => {
                self.keys(action).iter().map(Combo::parts).collect()
            }
            Keys::Hold(first, then) => match (self.keys(first).first(), self.keys(then).first()) {
                (Some(a), Some(b)) => vec![[a.parts(), b.parts()].concat()],
                _ => Vec::new(),
            },
            Keys::Pointer(parts) => vec![parts.to_vec()],
        }
    }

    pub fn explain(&self, text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some((before, after)) = rest.split_once('{') {
            out.push_str(before);
            let Some((name, tail)) = after.split_once('}') else {
                rest = after;
                break;
            };
            match Action::named(name) {
                Some(action) => out.push_str(&self.first_or_none(action)),
                None => out.push_str(name),
            }
            rest = tail;
        }
        out.push_str(rest);
        out
    }

    fn refuse_taken(&self, action: Action, combos: &[Combo]) -> Result<()> {
        for &combo in combos {
            if let Some(other) = self.owner(action.panel(), combo)
                && other != action
            {
                return Err(Error::usage(format!(
                    "{} belongs to {} ({}): give it another key first",
                    combo.shown(),
                    other.title(),
                    other.name(),
                )));
            }
        }
        Ok(())
    }

    pub fn set(&self, action: Action, combos: &[Combo]) -> Result<Custom> {
        self.refuse_taken(action, combos)?;
        let mut unique = Vec::new();
        for &combo in combos {
            if !unique.contains(&combo) {
                unique.push(combo);
            }
        }
        let mut custom = self.custom.clone();
        if unique == action.default_keys() {
            custom.remove(&action);
        } else {
            custom.insert(action, unique);
        }
        Ok(custom)
    }

    pub fn reset(&self, action: Option<Action>) -> Result<Custom> {
        let Some(action) = action else {
            return Ok(Custom::new());
        };
        self.refuse_taken(action, &action.default_keys())?;
        let mut custom = self.custom.clone();
        custom.remove(&action);
        Ok(custom)
    }
}

const fn own(
    group: Group,
    action: Action,
    keys: &'static [&'static str],
    title: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys: Keys::Own(action, keys),
        title,
        explanation,
    }
}

const fn same(
    group: Group,
    action: Action,
    title: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys: Keys::Same(action),
        title,
        explanation,
    }
}

const fn hold(
    group: Group,
    first: Action,
    then: Action,
    title: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys: Keys::Hold(first, then),
        title,
        explanation,
    }
}

const fn pointer(
    group: Group,
    parts: &'static [&'static str],
    title: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys: Keys::Pointer(parts),
        title,
        explanation,
    }
}

use Group::{App, Clips, Edit, Mark, Play, Trim};

pub const SHORTCUTS: &[Shortcut] = &[
    own(
        Play,
        Action::PlayOrStop,
        &["Space"],
        "Play or stop",
        Some(
            "In Dynamic Trim, it plays from 2 seconds before the playhead to 2 seconds after \
             and puts the playhead back, or stops a trim. While Ripple Delete Silence is \
             open, it plays the timeline as Remove would leave it.",
        ),
    ),
    own(
        Play,
        Action::PlayBackward,
        &["J"],
        "Play backward",
        Some("Press {play_backward} again to play backward faster."),
    ),
    own(
        Play,
        Action::Stop,
        &["K"],
        "Stop",
        Some(
            "Hold {stop} and tap {play_backward} or {play_forward} to step one frame. Hold \
             {stop}, then hold {play_backward} or {play_forward}, to play at half speed until \
             you let go.",
        ),
    ),
    own(
        Play,
        Action::PlayForward,
        &["L"],
        "Play forward",
        Some(
            "Press {play_forward} again to play faster. Editors keep three fingers on \
             {play_backward}, {stop} and {play_forward} to find a cut by eye and ear.",
        ),
    ),
    own(Play, Action::FrameBack, &["Left"], "One frame back", None),
    own(
        Play,
        Action::FrameForward,
        &["Right"],
        "One frame forward",
        None,
    ),
    own(
        Play,
        Action::SecondBack,
        &["Shift+Left"],
        "One second back",
        None,
    ),
    own(
        Play,
        Action::SecondForward,
        &["Shift+Right"],
        "One second forward",
        None,
    ),
    own(
        Play,
        Action::PreviousCut,
        &["Up"],
        "Previous cut",
        Some("In Dynamic Trim, it applies the trim and trims that cut next."),
    ),
    own(
        Play,
        Action::NextCut,
        &["Down"],
        "Next cut",
        Some("In Dynamic Trim, it applies the trim and trims that cut next."),
    ),
    own(
        Play,
        Action::PreviousMarker,
        &["Shift+Up"],
        "Previous marker",
        None,
    ),
    own(
        Play,
        Action::NextMarker,
        &["Shift+Down"],
        "Next marker",
        None,
    ),
    own(
        Play,
        Action::TimelineStart,
        &["Home"],
        "Start of the timeline",
        None,
    ),
    own(
        Play,
        Action::TimelineEnd,
        &["End"],
        "End of the timeline",
        None,
    ),
    own(
        Play,
        Action::GoToTimecode,
        &["="],
        "Go to a timecode",
        Some(
            "Type the digits after {go_to_timecode}. They fill from the right, so 100 goes to \
             00:00:01:00.",
        ),
    ),
    own(
        Play,
        Action::JumpForward,
        &["+"],
        "Jump forward",
        Some("Type how far after {jump_forward}, as for {go_to_timecode}: 100 jumps one second."),
    ),
    own(
        Play,
        Action::JumpBack,
        &["-"],
        "Jump back",
        Some("Type how far after {jump_back}, as for {go_to_timecode}."),
    ),
    own(
        Play,
        Action::TimelineOrSource,
        &["Q"],
        "Timeline or source in the viewer",
        Some(
            "The viewer shows one thing at a time: the timeline, or the source you pick clips \
             from. {timeline_or_source} switches to the one you looked at last.",
        ),
    ),
    own(
        Play,
        Action::AudioScrubbing,
        &["Shift+S"],
        "Audio scrubbing on or off",
        Some(
            "With scrubbing on, you hear the sound under the playhead as you drag it or step \
             frames. It helps to find a cut on a word.",
        ),
    ),
    own(
        Play,
        Action::Loop,
        &["Ctrl+/"],
        "Loop on or off",
        Some(
            "Playing past the end goes on from the start. With both In and Out marked, the \
             part between them plays over and over.",
        ),
    ),
    own(
        Play,
        Action::Skim,
        &["C"],
        "Skim",
        Some("Hold {skim} and move the pointer across the picture. The playhead follows."),
    ),
    own(
        Play,
        Action::FullScreen,
        &["Shift+F"],
        "Full screen",
        Some(
            "The viewer fills the display. Press {full_screen} again, or {leave_full_screen}, to \
             leave.",
        ),
    ),
    own(
        Play,
        Action::EnhancedViewer,
        &["Alt+F"],
        "Enhanced viewer",
        Some(
            "The viewer fills the page. Press {enhanced_viewer} again, or {leave_full_screen}, to \
             leave.",
        ),
    ),
    own(
        Play,
        Action::LeaveFullScreen,
        &["Escape"],
        "Leave full screen",
        Some("Also leaves the enhanced viewer."),
    ),
    own(
        Mark,
        Action::MarkIn,
        &["I"],
        "Mark In",
        Some(
            "Marks pick a part. In the source they pick the part of a clip to edit in. In the \
             timeline they pick the stretch to replace or delete.",
        ),
    ),
    own(Mark, Action::MarkOut, &["O"], "Mark Out", None),
    own(Mark, Action::GoToIn, &["Shift+I"], "Go to In", None),
    own(Mark, Action::GoToOut, &["Shift+O"], "Go to Out", None),
    own(
        Mark,
        Action::ClearMarks,
        &["Alt+X"],
        "Clear In and Out",
        None,
    ),
    own(
        Mark,
        Action::AddMarker,
        &["M"],
        "Add a marker",
        Some("On a marker, {add_marker} opens it for editing."),
    ),
    own(
        Mark,
        Action::DeleteMarker,
        &["Alt+M"],
        "Delete the marker",
        None,
    ),
    own(
        Edit,
        Action::SmartInsert,
        &["F9"],
        "Smart Insert",
        Some(
            "Puts the source in at the cut nearest the playhead, so you never split a shot by \
             accident. On Track 1 everything after it moves right to make room.",
        ),
    ),
    own(
        Edit,
        Action::Overwrite,
        &["F10"],
        "Overwrite",
        Some(
            "Writes the source over whatever is at the playhead. Nothing else moves, so the \
             timeline keeps its length.",
        ),
    ),
    own(
        Edit,
        Action::RippleOverwrite,
        &["Shift+F10"],
        "Ripple Overwrite",
        Some(
            "Swaps the clip under the playhead for the source, even when they're different \
             lengths. Everything after it moves, so nothing is cut short or left over. Use it \
             to try a better take.",
        ),
    ),
    own(
        Edit,
        Action::PlaceOnTop,
        &["F12"],
        "Place On Top",
        Some(
            "Puts the source on a track above, lined up with the playhead, and leaves the \
             story underneath alone. Use it for cutaways and B-roll.",
        ),
    ),
    own(
        Edit,
        Action::Append,
        &["Shift+F12"],
        "Append",
        Some("Adds the source after the last clip, wherever the playhead is."),
    ),
    own(
        Clips,
        Action::Split,
        &["Ctrl+B", "Ctrl+\\"],
        "Split at the playhead",
        Some(
            "It splits the selected clips and subtitles the playhead is over, or with nothing \
             selected, every clip there.",
        ),
    ),
    own(
        Clips,
        Action::Dissolve,
        &["Ctrl+T"],
        "Dissolve",
        Some(
            "A one-second dissolve on the selected cut, else on each cut of the selected clips, else \
             on Track 1's cut nearest the playhead: one picture fades out as the next fades in. Where the clips' sounds meet at the cut, \
             they cross fade. Next to a gap it fades from or to black. It is shorter where the clips have fewer frames past \
             the cut. {cut_side} picks whether it ends on the cut, starts on it or sits across it.",
        ),
    ),
    own(
        Clips,
        Action::Delete,
        &["Delete", "Backspace"],
        "Delete",
        Some(
            "Removes the selected clips and subtitles. On Track 1 the gap closes: Track 1 holds \
             the main story and never has holes. With volume keyframes selected, it removes only \
             them. With a dissolve selected, it removes the dissolve and its Cross Fade. After a \
             click in the Media Pool, it removes the selected media from the pool instead, after \
             asking.",
        ),
    ),
    own(
        Clips,
        Action::Lift,
        &["Alt+Delete", "Alt+Backspace"],
        "Lift",
        Some("Removes the selected clips and leaves the gap, so nothing after them moves."),
    ),
    own(
        Clips,
        Action::Copy,
        &["Ctrl+C"],
        "Copy",
        Some(
            "Copies the selected clips. After a click in the Keyframes tab or the keyframe tray, it copies the selected keyframes. Keyframes and clips share one clipboard.",
        ),
    ),
    own(
        Clips,
        Action::Cut,
        &["Ctrl+X"],
        "Cut",
        Some(
            "Copies the selected clips and removes them. On Track 1 the gap closes. After a click in the Keyframes tab or the keyframe tray, it cuts the selected keyframes.",
        ),
    ),
    own(
        Clips,
        Action::Paste,
        &["Ctrl+V"],
        "Paste",
        Some(
            "Pastes copied clips at the playhead on the clips' own track, over what is there. After a click in the Keyframes tab or the keyframe tray, it pastes copied keyframes on the selected clips: the first at the playhead, the others at their distance.",
        ),
    ),
    own(
        Clips,
        Action::PictureOff,
        &["D"],
        "Picture off or on",
        Some("The clip stays in place with its picture off, so you see what's under it."),
    ),
    own(
        Clips,
        Action::Transform,
        &["T"],
        "Transform",
        Some(
            "Shows the selected clip's handles in the viewer: drag inside to move it, a corner \
             to zoom, a side to stretch, the handle above or just outside a corner to turn it. \
             {transform} again hides them.",
        ),
    ),
    own(
        Clips,
        Action::Crop,
        &["Shift+T"],
        "Crop",
        Some("Shows a handle on each side of the selected clip in the viewer to crop it."),
    ),
    own(
        Clips,
        Action::DynamicZoom,
        &["Alt+T"],
        "Dynamic Zoom",
        Some(
            "Shows the selected clip's whole picture in the viewer with a green start box and \
             a red end box. Drag inside a box to move it and a corner to resize it; Shift \
             keeps a move to one axis. The clip zooms and pans from the start box to the end box.",
        ),
    ),
    pointer(
        Clips,
        &["Wheel"],
        "Zoom the clip's picture",
        Some(
            "While {transform}'s handles show, over the viewer's picture. A trackpad pinch zooms it too.",
        ),
    ),
    pointer(
        Clips,
        &["Alt", "Wheel"],
        "Turn the clip's picture",
        Some("While {transform}'s handles show, 1° a notch; with Shift too, 15°."),
    ),
    own(
        Clips,
        Action::VoiceIsolation,
        &["Alt+V"],
        "Voice Isolation on or off",
        Some("Takes background noise out of the selected clips' voices."),
    ),
    own(
        Clips,
        Action::Ducking,
        &["Alt+D"],
        "Ducking on or off",
        Some(
            "The selected clip's track dips under the speech of the other tracks with sound, \
             at the usual settings. The Mixer changes them.",
        ),
    ),
    own(
        Clips,
        Action::ChangeDuration,
        &["Ctrl+D"],
        "Change duration",
        Some("Of the selected clips, or of the selected dissolve, as a timecode or in frames."),
    ),
    own(
        Clips,
        Action::ChangeSpeed,
        &["R"],
        "Change speed",
        Some("A speed in percent, played backward, or one frame held still."),
    ),
    own(
        Clips,
        Action::EditSubtitle,
        &["Enter"],
        "Edit the selected subtitle",
        Some("Shows its text and position in the Inspector."),
    ),
    own(
        Clips,
        Action::NextKeyframe,
        &["]"],
        "Next keyframe",
        Some(
            "The playhead goes to the next volume keyframe of the selected clips. While Ripple \
             Delete Silence is open, {next_keyframe} goes to its next stripe instead.",
        ),
    ),
    own(
        Clips,
        Action::PreviousKeyframe,
        &["["],
        "Previous keyframe",
        Some("The playhead goes to the volume keyframe of the selected clips before it."),
    ),
    own(
        Clips,
        Action::AddKeyframe,
        &["Ctrl+["],
        "Add a keyframe",
        Some("On the selected rows of the Keyframes tab or the keyframe tray."),
    ),
    own(
        Clips,
        Action::DeleteKeyframe,
        &["Alt+]"],
        "Delete a keyframe",
        Some("On the selected rows of the Keyframes tab or the keyframe tray."),
    ),
    same(
        Clips,
        Action::NextKeyframe,
        "Next silence stripe",
        Some("While Ripple Delete Silence is open: the playhead goes to the next stripe."),
    ),
    same(
        Clips,
        Action::PreviousKeyframe,
        "Previous silence stripe",
        Some("While Ripple Delete Silence is open: the playhead goes to the stripe before."),
    ),
    own(
        Clips,
        Action::KeepStripe,
        &["X"],
        "Keep or cut the silence stripe at the playhead",
        Some(
            "While Ripple Delete Silence is open. A kept stripe turns gray and hatched and \
             stays when Remove cuts the red ones. A click on a stripe does the same, and its \
             edges drag to cut less or more.",
        ),
    ),
    own(
        Trim,
        Action::SelectAll,
        &["Ctrl+A"],
        "Select all",
        Some(
            "Every clip and subtitle on the timeline. After a click in the Media Pool, all it \
             shows.",
        ),
    ),
    own(
        Trim,
        Action::SelectNothing,
        &["Ctrl+Shift+A"],
        "Select nothing",
        None,
    ),
    own(
        Trim,
        Action::SelectAtPlayhead,
        &["Alt+Shift+V"],
        "Select the clips at the playhead",
        Some("Every clip under the playhead, on every track."),
    ),
    own(
        Trim,
        Action::NearestCut,
        &["V"],
        "Select the nearest cut",
        Some(
            "Selects the cut nearest the playhead, ready to trim with {trim_left} and \
             {trim_right}.",
        ),
    ),
    own(
        Trim,
        Action::CutSide,
        &["U"],
        "Switch the side of the cut",
        Some(
            "A cut has two sides: the end of the clip going out and the start of the one \
             coming in. {cut_side} goes from rolling both, which moves the cut and keeps the \
             length, to the clip coming in, to the clip going out. In Dynamic Trim too.",
        ),
    ),
    own(
        Trim,
        Action::TrimLeft,
        &[","],
        "Trim the cut a frame left",
        None,
    ),
    own(
        Trim,
        Action::TrimRight,
        &["."],
        "Trim the cut a frame right",
        None,
    ),
    own(
        Trim,
        Action::TrimSecondLeft,
        &["Shift+,"],
        "Trim the cut a second left",
        None,
    ),
    own(
        Trim,
        Action::TrimSecondRight,
        &["Shift+."],
        "Trim the cut a second right",
        None,
    ),
    own(
        Trim,
        Action::SelectSentence,
        &["X"],
        "Select the sentence",
        Some(
            "While the pointer is over the Transcription window: the sentence around a word, \
             with In and Out on it.",
        ),
    ),
    pointer(
        Trim,
        &["Click"],
        "Trim Editor",
        Some(
            "Click a cut: while it is selected and the player is stopped, the viewer shows \
             the last frame going out beside the first frame coming in, over their \
             filmstrips. Drag a filmstrip's handle, or use the -1 and +1 buttons beside the \
             transport, to trim the clip going out, the one coming in, or both. Click a \
             dissolve to see it between the filmstrips and drag its ends to change its \
             length. {play_or_stop} plays as usual; Esc closes it.",
        ),
    ),
    own(
        Trim,
        Action::DynamicTrim,
        &["W"],
        "Dynamic Trim",
        Some(
            "Trims with {play_backward}, {stop} and {play_forward}. Press {dynamic_trim} on a \
             selected cut, or on the cut nearest the playhead: the playhead turns yellow and \
             goes to the cut, and the Trim Editor shows it. {play_backward} and {play_forward} \
             now move the cut instead of the playhead, playing it with sound while the viewer \
             shows the frames on both sides, so you see and hear the timing change and stop \
             where it looks right. With clips selected instead, it slips or slides them: \
             right-click its button to choose. Press {dynamic_trim} again to finish.",
        ),
    ),
    same(
        Trim,
        Action::PlayForward,
        "Dynamic Trim forward",
        Some("Press {play_forward} again to trim faster: 2, 4, 8, 16, 32 and 64 times."),
    ),
    same(
        Trim,
        Action::PlayBackward,
        "Dynamic Trim backward",
        Some("Press {play_backward} again to trim faster."),
    ),
    same(
        Trim,
        Action::Stop,
        "Dynamic Trim: stop",
        Some(
            "Stops the trim where it is. A trim also stops where a clip runs out: the cut turns \
             red and the viewer says why.",
        ),
    ),
    hold(
        Trim,
        Action::Stop,
        Action::PlayForward,
        "Dynamic Trim one frame forward",
        Some("Hold {play_forward} too to trim at half speed until you let go."),
    ),
    hold(
        Trim,
        Action::Stop,
        Action::PlayBackward,
        "Dynamic Trim one frame backward",
        Some("Hold {play_backward} too to trim at half speed until you let go."),
    ),
    own(
        Trim,
        Action::Snapping,
        &["N"],
        "Snapping on or off",
        Some("Edges, markers and the playhead pull each other into line while you drag."),
    ),
    pointer(Trim, &["Ctrl", "Wheel"], "Zoom the timeline", None),
    own(
        App,
        Action::Undo,
        &["Ctrl+Z"],
        "Undo",
        Some("While Ripple Delete Silence is open, it undoes a change to its stripes or settings."),
    ),
    own(
        App,
        Action::Redo,
        &["Ctrl+Shift+Z"],
        "Redo",
        Some("While Ripple Delete Silence is open, it redoes a change to its stripes or settings."),
    ),
    own(App, Action::Import, &["Ctrl+I"], "Import media", None),
    own(App, Action::NewBin, &["Ctrl+Shift+N"], "New bin", None),
    own(App, Action::Preferences, &["Ctrl+,"], "Preferences", None),
    own(App, Action::Help, &["F1"], "Help", None),
];
