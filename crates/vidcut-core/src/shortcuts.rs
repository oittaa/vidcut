//! The keyboard shortcuts (spec §11), one action each, with a few plain
//! words where the action isn't obvious. The GUI's Keyboard Shortcuts
//! window (F1) shows them and `keys.list` returns them. Ctrl and Alt are
//! Cmd and Option on macOS.

use serde::Serialize;

/// What a shortcut is for, in the order the window lists them.
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

/// One action and the keys that do it, pressed together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shortcut {
    pub group: Group,
    pub keys: &'static [&'static str],
    pub action: &'static str,
    /// What it does and why, where the action's name doesn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<&'static str>,
}

/// A key's name on this system: Ctrl and Alt are Cmd and Option on macOS.
pub fn key_name(key: &str, mac: bool) -> &str {
    match (key, mac) {
        ("Ctrl", true) => "Cmd",
        ("Alt", true) => "Option",
        _ => key,
    }
}

const fn key(
    group: Group,
    keys: &'static [&'static str],
    action: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys,
        action,
        explanation,
    }
}

use Group::{App, Clips, Edit, Mark, Play, Trim};

pub const SHORTCUTS: &[Shortcut] = &[
    key(Play, &["Space"], "Play or stop", None),
    key(
        Play,
        &["J"],
        "Play backward",
        Some("Press J again to play backward faster."),
    ),
    key(
        Play,
        &["K"],
        "Stop",
        Some("Hold K and tap J or L to step one frame. Hold K with J or L to play slowly."),
    ),
    key(
        Play,
        &["L"],
        "Play forward",
        Some(
            "Press L again to play faster. Editors keep three fingers on J, K and L to find a \
             cut by eye and ear.",
        ),
    ),
    key(Play, &["Left"], "One frame back", None),
    key(Play, &["Right"], "One frame forward", None),
    key(Play, &["Shift", "Left"], "One second back", None),
    key(Play, &["Shift", "Right"], "One second forward", None),
    key(Play, &["Up"], "Previous cut", None),
    key(Play, &["Down"], "Next cut", None),
    key(Play, &["Shift", "Up"], "Previous marker", None),
    key(Play, &["Shift", "Down"], "Next marker", None),
    key(Play, &["Home"], "Start of the timeline", None),
    key(Play, &["End"], "End of the timeline", None),
    key(
        Play,
        &["="],
        "Go to a timecode",
        Some("Type the digits after =. They fill from the right, so 100 goes to 00:00:01:00."),
    ),
    key(
        Play,
        &["+"],
        "Jump forward",
        Some("Type how far after +, as for =: 100 jumps one second."),
    ),
    key(
        Play,
        &["-"],
        "Jump back",
        Some("Type how far after -, as for =."),
    ),
    key(
        Play,
        &["Q"],
        "Timeline or source in the viewer",
        Some(
            "The viewer shows one thing at a time: the timeline, or the source you pick clips \
             from. Q switches to the one you looked at last.",
        ),
    ),
    key(
        Play,
        &["Shift", "S"],
        "Audio scrubbing on or off",
        Some(
            "With scrubbing on, you hear the sound under the playhead as you drag it or step \
             frames. It helps to find a cut on a word.",
        ),
    ),
    key(
        Mark,
        &["I"],
        "Mark In",
        Some(
            "Marks pick a part. In the source they pick the part of a clip to edit in. In the \
             timeline they pick the stretch to replace or delete.",
        ),
    ),
    key(Mark, &["O"], "Mark Out", None),
    key(Mark, &["Shift", "I"], "Go to In", None),
    key(Mark, &["Shift", "O"], "Go to Out", None),
    key(Mark, &["Alt", "X"], "Clear In and Out", None),
    key(
        Mark,
        &["M"],
        "Add a marker",
        Some("On a marker, M opens it for editing."),
    ),
    key(Mark, &["Alt", "M"], "Delete the marker", None),
    key(
        Edit,
        &["F9"],
        "Smart Insert",
        Some(
            "Puts the source in at the cut nearest the playhead, so you never split a shot by \
             accident. On Track 1 everything after it moves right to make room.",
        ),
    ),
    key(
        Edit,
        &["F10"],
        "Overwrite",
        Some(
            "Writes the source over whatever is at the playhead. Nothing else moves, so the \
             timeline keeps its length.",
        ),
    ),
    key(
        Edit,
        &["Shift", "F10"],
        "Ripple Overwrite",
        Some(
            "Swaps the clip under the playhead for the source, even when they're different \
             lengths. Everything after it moves, so nothing is cut short or left over. Use it \
             to try a better take.",
        ),
    ),
    key(
        Edit,
        &["F12"],
        "Place On Top",
        Some(
            "Puts the source on a track above, lined up with the playhead, and leaves the \
             story underneath alone. Use it for cutaways and B-roll.",
        ),
    ),
    key(
        Edit,
        &["Shift", "F12"],
        "Append",
        Some("Adds the source after the last clip, wherever the playhead is."),
    ),
    key(
        Clips,
        &["Ctrl", "B"],
        "Split at the playhead",
        Some("Ctrl+\\ does the same."),
    ),
    key(
        Clips,
        &["Delete"],
        "Delete",
        Some(
            "Removes the selected clips. On Track 1 the gap closes: Track 1 holds the main \
             story and never has holes. Backspace does the same.",
        ),
    ),
    key(
        Clips,
        &["Alt", "Delete"],
        "Lift",
        Some("Removes the selected clips and leaves the gap, so nothing after them moves."),
    ),
    key(Clips, &["Ctrl", "C"], "Copy", None),
    key(
        Clips,
        &["Ctrl", "X"],
        "Cut",
        Some("Copies the selected clips and removes them. On Track 1 the gap closes."),
    ),
    key(
        Clips,
        &["Ctrl", "V"],
        "Paste",
        Some("Pastes at the playhead on the clips' own track, over what is there."),
    ),
    key(
        Clips,
        &["D"],
        "Picture off or on",
        Some("The clip stays in place with its picture off, so you see what's under it."),
    ),
    key(Clips, &["Ctrl", "D"], "Change duration", None),
    key(
        Clips,
        &["R"],
        "Change speed",
        Some("A speed in percent, played backward, or one frame held still."),
    ),
    key(Trim, &["Ctrl", "A"], "Select all", None),
    key(Trim, &["Ctrl", "Shift", "A"], "Select nothing", None),
    key(
        Trim,
        &["Alt", "Shift", "V"],
        "Select the clips at the playhead",
        Some("Every clip under the playhead, on every track."),
    ),
    key(
        Trim,
        &["V"],
        "Select the nearest cut",
        Some("Selects the cut nearest the playhead, ready to trim with comma and period."),
    ),
    key(
        Trim,
        &["U"],
        "Switch the side of the cut",
        Some(
            "A cut has two sides: the end of the clip going out and the start of the one \
             coming in. U switches between trimming one side and rolling both, which moves \
             the cut and keeps the length.",
        ),
    ),
    key(
        Trim,
        &[","],
        "Trim the cut a frame left",
        Some("Hold Shift to trim a second."),
    ),
    key(
        Trim,
        &["."],
        "Trim the cut a frame right",
        Some("Hold Shift to trim a second."),
    ),
    key(
        Trim,
        &["W"],
        "Dynamic Trim",
        Some(
            "Trims while the video plays. Select a cut and press W: the playhead turns yellow, \
             and J, K and L now move the cut instead of the playhead. You feel the timing in \
             motion and stop where it looks right. Right-click its button to slip or slide \
             clips instead. Press W again to finish.",
        ),
    ),
    key(
        Trim,
        &["N"],
        "Snapping on or off",
        Some("Edges, markers and the playhead pull each other into line while you drag."),
    ),
    key(Trim, &["Ctrl", "Wheel"], "Zoom the timeline", None),
    key(App, &["Ctrl", "Z"], "Undo", None),
    key(App, &["Ctrl", "Shift", "Z"], "Redo", None),
    key(App, &["Ctrl", "I"], "Import media", None),
    key(App, &["Ctrl", "Shift", "N"], "New bin", None),
    key(App, &["Ctrl", ","], "Preferences", None),
    key(App, &["F1"], "Keyboard shortcuts", None),
];
